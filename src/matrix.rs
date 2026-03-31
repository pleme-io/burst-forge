//! Scaling matrix — run multiple scenarios, collect results.

use std::time::Duration;

use chrono::Utc;

use crate::burst;
use crate::config::Config;
use crate::kubectl::KubeCtl;
use crate::nodes;
use crate::types::{MatrixReport, ScenarioResult};
use crate::verify;

/// Run the scaling matrix: iterate scenarios, patch replicas, verify, burst.
///
/// # Errors
///
/// Returns an error if:
/// - No scenarios are configured or matched
/// - Node scaling fails
/// - `DaemonSet` warmup fails (when configured)
/// - All scenarios completed but any had errors
///
/// Node scale-down is always attempted in cleanup, even on error.
#[allow(clippy::too_many_lines)]
pub fn run_matrix(
    kubectl: &KubeCtl,
    config: &Config,
    scenario_filter: Option<&str>,
    skip_scaling: bool,
) -> anyhow::Result<MatrixReport> {
    let timestamp = Utc::now().to_rfc3339();

    if config.scenarios.is_empty() {
        anyhow::bail!("No scenarios configured. Add scenarios to your burst-forge.yaml config.");
    }

    let scenarios: Vec<_> = config
        .scenarios
        .iter()
        .filter(|s| {
            scenario_filter
                .is_none_or(|f| s.name == f)
        })
        .collect();

    if scenarios.is_empty() {
        anyhow::bail!(
            "No scenarios matched filter {scenario_filter:?}"
        );
    }

    // Scale up node group if configured — failure is fatal
    if let Some(ng) = &config.node_group {
        let max_nodes_needed = scenarios
            .iter()
            .map(|s| {
                s.nodes
                    .unwrap_or_else(|| nodes::calculate_nodes(s.replicas, ng.pods_per_node))
            })
            .max()
            .unwrap_or(1)
            .min(ng.max_nodes);

        println!("\n=== Node Group Pre-Heat: {max_nodes_needed} nodes ===\n");

        // Node scaling failure is fatal — cannot run burst tests without nodes
        nodes::scale_node_group(ng, max_nodes_needed)
            .map_err(|e| anyhow::anyhow!("FATAL: Failed to scale node group to {max_nodes_needed}: {e}"))?;

        nodes::wait_for_nodes(
            kubectl,
            max_nodes_needed,
            Duration::from_secs(config.timeout_secs),
            Duration::from_secs(config.node_poll_interval_secs),
        )
        .map_err(|e| anyhow::anyhow!("FATAL: Nodes did not become ready: {e}"))?;

        nodes::tag_nodes(kubectl, "burst-forge=true")?;

        // Wait for warmup DaemonSet if configured — failure is fatal
        if let Some(warmup) = &config.warmup_daemonset {
            println!("\n=== Waiting for warmup DaemonSet {}/{}  ===\n", warmup.namespace, warmup.name);
            nodes::wait_for_daemonset_ready(
                kubectl,
                &warmup.namespace,
                &warmup.name,
                Duration::from_secs(warmup.timeout_secs),
            )
            .map_err(|e| anyhow::anyhow!(
                "FATAL: Warmup DaemonSet {}/{} did not become ready within {}s: {e}",
                warmup.namespace,
                warmup.name,
                warmup.timeout_secs,
            ))?;
        }
    }

    println!(
        "\n=== Scaling Matrix: {} scenarios ===\n",
        scenarios.len()
    );

    let mut results = Vec::new();

    for (i, scenario) in scenarios.iter().enumerate() {
        println!(
            "\n--- Scenario: {} (replicas={}, gw={}, wh={}) ---",
            scenario.name,
            scenario.replicas,
            scenario.gateway_replicas,
            scenario.webhook_replicas,
        );

        let result = run_single_scenario(kubectl, config, scenario, skip_scaling);
        results.push(result);

        // Cool down between scenarios (skip after last)
        if i < scenarios.len() - 1 {
            println!("\n  Cooling down {}s...", config.cooldown_secs);
            std::thread::sleep(std::time::Duration::from_secs(config.cooldown_secs));
        }
    }

    // Always attempt cleanup, regardless of scenario results

    // Resume HelmReleases and reset replicas after all scenarios
    if !skip_scaling {
        println!("\n  Resetting deployments to 1 replica...");
        let _ = kubectl.run(&[
            "-n", &config.injection_namespace, "scale", "deployment",
            &config.gateway_deployment, "--replicas=1",
        ]);
        let _ = kubectl.run(&[
            "-n", &config.injection_namespace, "scale", "deployment",
            &config.webhook_deployment, "--replicas=1",
        ]);

        println!("  Resuming HelmReleases (FluxCD takes control again)...");
        let _ = kubectl.run(&[
            "-n", &config.injection_namespace, "patch", "helmrelease",
            &config.gateway_release, "--type=merge",
            "-p", r#"{"spec":{"suspend":false}}"#,
        ]);
        let _ = kubectl.run(&[
            "-n", &config.injection_namespace, "patch", "helmrelease",
            &config.webhook_release, "--type=merge",
            "-p", r#"{"spec":{"suspend":false}}"#,
        ]);

        // Legacy cleanup path (in case old code path is hit)
        if let Err(e) = kubectl.patch_helmrelease_replicas(
            &config.injection_namespace,
            &config.gateway_release,
            1,
        ) {
            eprintln!("  WARNING: Failed to reset gateway replicas: {e}");
        }
        if let Err(e) = kubectl.patch_helmrelease_replicas(
            &config.injection_namespace,
            &config.webhook_release,
            1,
        ) {
            eprintln!("  WARNING: Failed to reset webhook replicas: {e}");
        }
    }

    // Scale node group back to 0 after all scenarios — always attempt
    if let Some(ng) = &config.node_group {
        println!("\n=== Scaling node group back to 0 ===\n");
        if let Err(e) = nodes::scale_node_group(ng, 0) {
            eprintln!("  WARNING: Failed to scale down node group: {e}");
        }
    }

    // Check if any scenario had errors — collect failure info before moving results
    let failure_count = results.iter().filter(|r| r.error.is_some()).count();
    let total_count = results.len();
    let failure_summary: Vec<String> = results
        .iter()
        .filter(|r| r.error.is_some())
        .map(|r| {
            format!(
                "  - {}: {}",
                r.name,
                r.error.as_deref().unwrap_or("unknown error")
            )
        })
        .collect();

    let report = MatrixReport {
        timestamp,
        scenarios: results,
    };

    if failure_count > 0 {
        // Still print the report JSON so the caller has the data
        println!("\n=== MATRIX REPORT (with failures) ===");
        if let Ok(json) = serde_json::to_string_pretty(&report) {
            println!("{json}");
        }

        anyhow::bail!(
            "Matrix completed with {failure_count}/{total_count} scenario failures:\n{}",
            failure_summary.join("\n")
        );
    }

    Ok(report)
}

fn make_error_result(scenario: &crate::config::Scenario, error: String) -> ScenarioResult {
    ScenarioResult {
        name: scenario.name.clone(),
        replicas: scenario.replicas,
        gateway_replicas: scenario.gateway_replicas,
        webhook_replicas: scenario.webhook_replicas,
        verify: None,
        burst: None,
        error: Some(error),
    }
}

/// Run a single scenario and capture the result.
fn run_single_scenario(
    kubectl: &KubeCtl,
    config: &Config,
    scenario: &crate::config::Scenario,
    skip_scaling: bool,
) -> ScenarioResult {
    // Scale infrastructure (unless skipping)
    if !skip_scaling {
        // Suspend HelmReleases so FluxCD doesn't revert our replica changes
        println!("  Suspending HelmReleases (prevent FluxCD revert)...");
        let _ = kubectl.run(&[
            "-n", &config.injection_namespace, "patch", "helmrelease",
            &config.gateway_release, "--type=merge",
            "-p", r#"{"spec":{"suspend":true}}"#,
        ]);
        let _ = kubectl.run(&[
            "-n", &config.injection_namespace, "patch", "helmrelease",
            &config.webhook_release, "--type=merge",
            "-p", r#"{"spec":{"suspend":true}}"#,
        ]);

        // Patch the underlying Deployments directly (not HelmRelease values)
        // This avoids the FluxCD reconciliation race entirely
        println!("  Scaling gateway to {} replicas...", scenario.gateway_replicas);
        if let Err(e) = kubectl.run(&[
            "-n", &config.injection_namespace, "scale", "deployment",
            &config.gateway_deployment,
            &format!("--replicas={}", scenario.gateway_replicas),
        ]) {
            return make_error_result(scenario, format!("Failed to scale gateway: {e}"));
        }

        println!("  Scaling webhook to {} replicas...", scenario.webhook_replicas);
        if let Err(e) = kubectl.run(&[
            "-n", &config.injection_namespace, "scale", "deployment",
            &config.webhook_deployment,
            &format!("--replicas={}", scenario.webhook_replicas),
        ]) {
            return make_error_result(scenario, format!("Failed to scale webhook: {e}"));
        }

        // Wait for rollout to complete — pods must be READY, not just created
        let gw_deploy_path = format!("deployment/{}", config.gateway_deployment);
        println!("  Waiting for gateway rollout...");
        let _ = kubectl.run(&[
            "-n", &config.injection_namespace, "rollout", "status",
            &gw_deploy_path,
            &format!("--timeout={}s", config.rollout_wait_secs),
        ]);
        let wh_deploy_path = format!("deployment/{}", config.webhook_deployment);
        println!("  Waiting for webhook rollout...");
        let _ = kubectl.run(&[
            "-n", &config.injection_namespace, "rollout", "status",
            &wh_deploy_path,
            &format!("--timeout={}s", config.rollout_wait_secs),
        ]);

        // Verify actual replica counts
        let gw_ready = kubectl.run(&[
            "-n", &config.injection_namespace, "get", "deployment",
            &config.gateway_deployment,
            "-o", "jsonpath={.status.readyReplicas}",
        ]).unwrap_or_default();
        let wh_ready = kubectl.run(&[
            "-n", &config.injection_namespace, "get", "deployment",
            &config.webhook_deployment,
            "-o", "jsonpath={.status.readyReplicas}",
        ]).unwrap_or_default();
        println!("  Gateway ready: {gw_ready}/{}, Webhook ready: {wh_ready}/{}",
            scenario.gateway_replicas, scenario.webhook_replicas);
    }

    // Verify infrastructure
    let verify_result = match verify::verify_infra(kubectl, config) {
        Ok(v) => Some(v),
        Err(e) => {
            return ScenarioResult {
                name: scenario.name.clone(),
                replicas: scenario.replicas,
                gateway_replicas: scenario.gateway_replicas,
                webhook_replicas: scenario.webhook_replicas,
                verify: None,
                burst: None,
                error: Some(format!("Verification failed: {e}")),
            };
        }
    };

    // Run burst
    let burst_result = match burst::run_burst(kubectl, config, scenario.replicas, 1) {
        Ok(b) => Some(b),
        Err(e) => {
            return ScenarioResult {
                name: scenario.name.clone(),
                replicas: scenario.replicas,
                gateway_replicas: scenario.gateway_replicas,
                webhook_replicas: scenario.webhook_replicas,
                verify: verify_result,
                burst: None,
                error: Some(format!("Burst failed: {e}")),
            };
        }
    };

    ScenarioResult {
        name: scenario.name.clone(),
        replicas: scenario.replicas,
        gateway_replicas: scenario.gateway_replicas,
        webhook_replicas: scenario.webhook_replicas,
        verify: verify_result,
        burst: burst_result,
        error: None,
    }
}
