//! Infrastructure verification checks.

use crate::config::Config;
use crate::kubectl::KubeCtl;
use crate::types::{ImageCacheCheck, ImageStatus, VerifyResult};

/// Verify that the cluster infrastructure is ready for burst testing.
///
/// Checks nodes, Akeyless gateway, injection webhook, burst deployment,
/// and optionally the image cache (Zot registry).
///
/// # Errors
///
/// Returns an error if critical infrastructure components are missing.
pub fn verify_infra(kubectl: &KubeCtl, config: &Config) -> anyhow::Result<VerifyResult> {
    println!("Verifying infrastructure...");

    // Check nodes
    let nodes = kubectl.run(&["get", "nodes", "--no-headers"])?;
    let node_count = nodes.lines().count();
    let ready_nodes = nodes.lines().filter(|l| l.contains("Ready")).count();
    println!("  Nodes: {ready_nodes}/{node_count} Ready");

    // Check Akeyless gateway
    let gw = kubectl.run(&[
        "-n",
        &config.akeyless_namespace,
        "get",
        "pods",
        "-l",
        &config.gateway_label,
        "--no-headers",
    ])?;
    let gateway_replicas = gw.lines().filter(|l| l.contains("Running")).count();
    println!("  Akeyless gateway: {gateway_replicas} Running");

    // Check injection webhook
    let inj = kubectl.run(&[
        "-n",
        &config.akeyless_namespace,
        "get",
        "pods",
        "-l",
        &config.webhook_label,
        "--no-headers",
    ])?;
    let webhook_replicas = inj.lines().filter(|l| l.contains("Running")).count();
    println!("  Injection webhook: {webhook_replicas} Running");

    // Check deployment exists
    let deployment_found = match kubectl.run(&[
        "-n",
        &config.namespace,
        "get",
        "deployment",
        &config.deployment,
        "--no-headers",
    ]) {
        Ok(d) => {
            println!("  Deployment: {d}");
            true
        }
        Err(e) => {
            println!("  Deployment: NOT FOUND ({e})");
            false
        }
    };

    // Check image cache if configured
    let image_cache = check_image_cache(kubectl, config);

    if gateway_replicas == 0 || webhook_replicas == 0 {
        anyhow::bail!(
            "Infrastructure not ready: gateway={gateway_replicas}, webhook={webhook_replicas}"
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

    println!("Infrastructure verified.");
    Ok(result)
}

/// Check the image cache (Zot registry) for required images.
///
/// Runs `kubectl exec` into a Zot pod and queries the registry API.
fn check_image_cache(kubectl: &KubeCtl, config: &Config) -> Option<ImageCacheCheck> {
    let registry = config.cache_registry.as_ref()?.clone();

    if config.required_images.is_empty() {
        return None;
    }

    println!("  Checking image cache ({registry})...");

    // Find a Zot pod
    let zot_pods = kubectl.run(&[
        "-n",
        "image-cache",
        "get",
        "pods",
        "-l",
        "app.kubernetes.io/name=zot",
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
        println!("    No Zot pod found, skipping image cache check");
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
            "image-cache",
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

        let status = if cached { "CACHED" } else { "MISSING" };
        println!("    {image_ref}: {status}");
        images.push(ImageStatus {
            image: image_ref.clone(),
            cached,
            tags,
        });
    }

    let all_cached = images.iter().all(|i| i.cached);
    if !all_cached {
        println!("  WARNING: Not all required images are cached");
    }

    Some(ImageCacheCheck { registry, images })
}
