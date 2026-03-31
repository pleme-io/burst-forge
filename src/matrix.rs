//! Scaling matrix — run multiple scenarios, collect results.

use std::time::Duration;

use chrono::Utc;

use crate::burst;
use crate::config::Config;
use crate::drain;
use crate::gates;
use crate::kubectl::KubeCtl;
use crate::nodes;
use crate::output;
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

        output::print_phase(&format!("Node Pre-Heat ({max_nodes_needed} nodes)"));

        // Node scaling failure is fatal — cannot run burst tests without nodes
        nodes::scale_node_group(ng, max_nodes_needed)
            .map_err(|e| anyhow::anyhow!("FATAL: Failed to scale node group to {max_nodes_needed}: {e}"))?;

        // [Gate 1] Node Ready Gate — wait for Ready+Schedulable nodes
        let gate1 = gates::wait_for_ready_schedulable_nodes(
            kubectl,
            max_nodes_needed,
            Duration::from_secs(config.timeout_secs),
            Duration::from_secs(config.node_poll_interval_secs),
        )?;
        gates::enforce(&gate1, config.strict_gates)?;

        nodes::tag_nodes(kubectl, "burst-forge=true")?;

        // [Gate 2] Warmup Gate — DaemonSet matches schedulable node count
        if let Some(warmup) = &config.warmup_daemonset {
            output::print_phase(&format!(
                "Warmup DaemonSet {}/{}", warmup.namespace, warmup.name
            ));
            let gate2 = gates::check_warmup_gate(
                kubectl,
                &warmup.namespace,
                &warmup.name,
                Duration::from_secs(warmup.timeout_secs),
            )?;
            gates::enforce(&gate2, config.strict_gates)?;
        }
    }

    output::print_phase(&format!("Scaling Matrix: {} scenarios", scenarios.len()));

    let mut results = Vec::new();

    for (i, scenario) in scenarios.iter().enumerate() {
        output::print_scenario(
            i,
            scenarios.len(),
            &scenario.name,
            scenario.replicas,
            scenario.gateway_replicas,
            scenario.webhook_replicas,
        );

        let result = run_single_scenario(kubectl, config, scenario, skip_scaling);
        results.push(result);

        // Inter-scenario cleanup (skip after last)
        if i < scenarios.len() - 1 {
            output::print_inter_scenario_cleanup();

            // Scale burst deployment to 0
            output::print_action("Scaling deployment to 0...");
            let _ = kubectl.run(&[
                "-n", &config.namespace, "scale", "deployment",
                &config.deployment, "--replicas=0",
            ]);

            // Wait for complete pod drain (verified 0 pods)
            let app_label = config.resolved_pod_label();
            if let Err(e) = drain::wait_for_zero_pods(kubectl, config, &app_label) {
                output::print_warning(&format!("Inter-scenario drain failed: {e}"));
            }

            // [Gate 5] Drain Gate — verify 0 pods AND infrastructure recovery
            // Use the current scenario's expected gateway/webhook counts
            let gate5 = gates::check_drain_gate(
                kubectl,
                config,
                scenario.gateway_replicas,
                scenario.webhook_replicas,
            )?;
            gates::enforce(&gate5, config.strict_gates)?;

            // Cooldown AFTER drain completes (not during)
            output::print_cooldown(config.cooldown_secs);
            std::thread::sleep(std::time::Duration::from_secs(config.cooldown_secs));
        }
    }

    // Always attempt cleanup, regardless of scenario results

    // Build summary table from results
    let summary_rows: Vec<output::SummaryRow> = results
        .iter()
        .map(|r| {
            output::build_summary_row(
                &r.name,
                r.replicas,
                r.burst.as_ref().map(|b| b.pods_running),
                r.burst.as_ref().and_then(|b| b.time_to_all_ready_ms),
                r.burst.as_ref().map(|b| b.injection_success_rate),
                r.error.is_some(),
            )
        })
        .collect();
    output::print_matrix_summary(&summary_rows);

    // Resume HelmReleases and reset replicas after all scenarios
    output::print_matrix_cleanup(skip_scaling);
    if !skip_scaling {
        let _ = kubectl.run(&[
            "-n", &config.injection_namespace, "scale", "deployment",
            &config.gateway_deployment, "--replicas=1",
        ]);
        let _ = kubectl.run(&[
            "-n", &config.injection_namespace, "scale", "deployment",
            &config.webhook_deployment, "--replicas=1",
        ]);

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
            output::print_warning(&format!("Failed to reset gateway replicas: {e}"));
        }
        if let Err(e) = kubectl.patch_helmrelease_replicas(
            &config.injection_namespace,
            &config.webhook_release,
            1,
        ) {
            output::print_warning(&format!("Failed to reset webhook replicas: {e}"));
        }
    }

    // Scale node group back to 0 after all scenarios — always attempt
    if let Some(ng) = &config.node_group {
        output::print_phase("Scaling Node Group to 0");
        if let Err(e) = nodes::scale_node_group(ng, 0) {
            output::print_warning(&format!("Failed to scale down node group: {e}"));
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
        output::print_phase("Matrix Report (with failures)");
        if let Ok(json) = serde_json::to_string_pretty(&report) {
            println!("{json}");
        }

        output::print_matrix_failures(failure_count, total_count, &failure_summary);

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
#[allow(clippy::too_many_lines)]
fn run_single_scenario(
    kubectl: &KubeCtl,
    config: &Config,
    scenario: &crate::config::Scenario,
    skip_scaling: bool,
) -> ScenarioResult {
    // Scale infrastructure (unless skipping)
    if !skip_scaling {
        // Suspend HelmReleases so FluxCD doesn't revert our replica changes
        output::print_action("Suspending HelmReleases (prevent FluxCD revert)...");
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
        output::print_action(&format!("Scaling gateway to {} replicas...", scenario.gateway_replicas));
        if let Err(e) = kubectl.run(&[
            "-n", &config.injection_namespace, "scale", "deployment",
            &config.gateway_deployment,
            &format!("--replicas={}", scenario.gateway_replicas),
        ]) {
            return make_error_result(scenario, format!("Failed to scale gateway: {e}"));
        }

        output::print_action(&format!("Scaling webhook to {} replicas...", scenario.webhook_replicas));
        if let Err(e) = kubectl.run(&[
            "-n", &config.injection_namespace, "scale", "deployment",
            &config.webhook_deployment,
            &format!("--replicas={}", scenario.webhook_replicas),
        ]) {
            return make_error_result(scenario, format!("Failed to scale webhook: {e}"));
        }

        // Wait for rollout to complete — pods must be READY, not just created
        let gw_deploy_path = format!("deployment/{}", config.gateway_deployment);
        output::print_action("Waiting for gateway rollout...");
        let _ = kubectl.run(&[
            "-n", &config.injection_namespace, "rollout", "status",
            &gw_deploy_path,
            &format!("--timeout={}s", config.rollout_wait_secs),
        ]);
        let wh_deploy_path = format!("deployment/{}", config.webhook_deployment);
        output::print_action("Waiting for webhook rollout...");
        let _ = kubectl.run(&[
            "-n", &config.injection_namespace, "rollout", "status",
            &wh_deploy_path,
            &format!("--timeout={}s", config.rollout_wait_secs),
        ]);

        // [Gate 3] Infrastructure Gate — readyReplicas == expected for both GW and WH
        match gates::check_infrastructure_gate(
            kubectl,
            config,
            scenario.gateway_replicas,
            scenario.webhook_replicas,
        ) {
            Ok(gate3) => {
                if let Err(e) = gates::enforce(&gate3, config.strict_gates) {
                    return make_error_result(
                        scenario,
                        format!(
                            "{e}. Infrastructure is not ready -- refusing to run burst with partial capacity."
                        ),
                    );
                }
            }
            Err(e) => {
                return make_error_result(
                    scenario,
                    format!("[Gate 3] kubectl error during infrastructure gate: {e}"),
                );
            }
        }
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

    // [Gate 4] Starting Line Gate — before burst
    match gates::check_starting_line_gate(
        kubectl,
        config,
        scenario.gateway_replicas,
        scenario.webhook_replicas,
    ) {
        Ok(gate4) => {
            if let Err(e) = gates::enforce(&gate4, config.strict_gates) {
                return make_error_result(scenario, format!("{e}"));
            }
        }
        Err(e) => {
            return make_error_result(
                scenario,
                format!("[Gate 4] kubectl error during starting line gate: {e}"),
            );
        }
    }

    // Run burst with gateway/webhook expected counts for starting-line verification
    let burst_result = match burst::run_burst(
        kubectl,
        config,
        scenario.replicas,
        1,
        scenario.gateway_replicas,
        scenario.webhook_replicas,
    ) {
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
