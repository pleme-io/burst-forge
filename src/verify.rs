//! Infrastructure verification checks.

use crate::config::Config;
use crate::kubectl::KubeCtl;
use crate::output;
use crate::types::{ImageCacheCheck, ImageStatus, VerifyResult};

/// Verify that the cluster infrastructure is ready for burst testing.
///
/// Checks nodes, injection gateway, injection webhook, burst deployment,
/// and optionally the image cache (Zot registry).
///
/// # Errors
///
/// Returns an error if critical infrastructure components are missing.
pub fn verify_infra(kubectl: &KubeCtl, config: &Config) -> anyhow::Result<VerifyResult> {
    output::print_verify_header();

    // Check nodes
    let nodes = kubectl.run(&["get", "nodes", "--no-headers"])?;
    let node_count = nodes.lines().count();
    let ready_nodes = nodes.lines().filter(|l| l.contains("Ready")).count();
    output::print_verify_check(
        "Nodes",
        &format!("{ready_nodes}/{node_count} Ready"),
        ready_nodes > 0,
    );

    if node_count == 0 {
        anyhow::bail!("No nodes found in cluster. Is kubeconfig correct?");
    }
    if ready_nodes == 0 {
        anyhow::bail!("No nodes are Ready. Cluster is not usable.");
    }

    // Check injection gateway
    let gw = kubectl.run(&[
        "-n",
        &config.injection_namespace,
        "get",
        "pods",
        "-l",
        &config.gateway_label,
        "--no-headers",
    ])?;
    let gateway_replicas = gw.lines().filter(|l| l.contains("Running")).count();
    output::print_verify_check(
        "Injection Gateway",
        &format!("{gateway_replicas} Running"),
        gateway_replicas > 0,
    );

    // Check injection webhook
    let inj = kubectl.run(&[
        "-n",
        &config.injection_namespace,
        "get",
        "pods",
        "-l",
        &config.webhook_label,
        "--no-headers",
    ])?;
    let webhook_replicas = inj.lines().filter(|l| l.contains("Running")).count();
    output::print_verify_check(
        "Injection Webhook",
        &format!("{webhook_replicas} Running"),
        webhook_replicas > 0,
    );

    // Check deployment exists
    let deployment_found = match kubectl.run(&[
        "-n",
        &config.namespace,
        "get",
        "deployment",
        &config.deployment,
        "--no-headers",
    ]) {
        Ok(_d) => {
            output::print_verify_check("Deployment", &config.deployment, true);
            true
        }
        Err(_e) => {
            output::print_verify_check("Deployment", "NOT FOUND", false);
            false
        }
    };

    // Check image cache if configured
    let image_cache = check_image_cache(kubectl, config);

    if gateway_replicas == 0 {
        anyhow::bail!(
            "Injection gateway has 0 running pods (ns={}, label={}). \
             Check that the gateway HelmRelease '{}' is deployed.",
            config.injection_namespace,
            config.gateway_label,
            config.gateway_release,
        );
    }
    if webhook_replicas == 0 {
        anyhow::bail!(
            "Injection webhook has 0 running pods (ns={}, label={}). \
             Check that the webhook HelmRelease '{}' is deployed.",
            config.injection_namespace,
            config.webhook_label,
            config.webhook_release,
        );
    }

    let result = VerifyResult {
        node_count,
        ready_nodes,
        gateway_replicas,
        webhook_replicas,
        deployment_found,
        image_cache,
    };

    output::print_verify_complete();
    Ok(result)
}

/// Check the image cache (Zot registry) for required images.
///
/// Uses config-driven namespace and label selector.
fn check_image_cache(kubectl: &KubeCtl, config: &Config) -> Option<ImageCacheCheck> {
    let registry = config.resolved_cache_registry()?;

    if config.required_images.is_empty() {
        return None;
    }

    let ic_namespace = config.image_cache_namespace();
    let ic_label = config.image_cache_label();

    output::print_action(&format!("Checking image cache ({registry}) in ns={ic_namespace} label={ic_label}..."));

    // Find a Zot pod
    let zot_pods = kubectl.run(&[
        "-n",
        &ic_namespace,
        "get",
        "pods",
        "-l",
        &ic_label,
        "--no-headers",
        "-o",
        "custom-columns=NAME:.metadata.name,STATUS:.status.phase",
    ]);

    let zot_pod = match zot_pods {
        Ok(output) => {
            output
                .lines()
                .find(|l| l.contains("Running"))
                .and_then(|l| l.split_whitespace().next())
                .map(String::from)
        }
        Err(_) => None,
    };

    let Some(pod_name) = zot_pod else {
        output::print_warning(&format!("No Zot pod found in {ic_namespace} (label={ic_label}), skipping image cache check"));
        return Some(ImageCacheCheck {
            registry,
            images: config
                .required_images
                .iter()
                .map(|img| ImageStatus {
                    image: img.clone(),
                    cached: false,
                    tags: Vec::new(),
                })
                .collect(),
        });
    };

    let mut images = Vec::new();
    for image_ref in &config.required_images {
        // Split image:tag into image and tag
        let (image_name, expected_tag) = match image_ref.rsplit_once(':') {
            Some((name, tag)) => (name, Some(tag)),
            None => (image_ref.as_str(), None),
        };

        // Query the Zot registry API via kubectl exec
        let result = kubectl.run(&[
            "-n",
            &ic_namespace,
            "exec",
            &pod_name,
            "--",
            "wget",
            "-q",
            "-O-",
            &format!("http://localhost:5000/v2/{image_name}/tags/list"),
        ]);

        let (cached, tags) = match result {
            Ok(output) => {
                let parsed: serde_json::Value =
                    serde_json::from_str(&output).unwrap_or(serde_json::Value::Null);
                let tags: Vec<String> = parsed["tags"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();

                let cached = match expected_tag {
                    Some(tag) => tags.iter().any(|t| t == tag),
                    None => !tags.is_empty(),
                };
                (cached, tags)
            }
            Err(_) => (false, Vec::new()),
        };

        let status_str = if cached {
            output::green("CACHED")
        } else {
            output::red("MISSING")
        };
        output::print_status(&format!("  {image_ref}: {status_str}"));
        images.push(ImageStatus {
            image: image_ref.clone(),
            cached,
            tags,
        });
    }

    let all_cached = images.iter().all(|i| i.cached);
    if !all_cached {
        output::print_warning("Not all required images are cached");
    }

    Some(ImageCacheCheck { registry, images })
}
