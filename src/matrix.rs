//! Scaling matrix — run multiple scenarios, collect results.

use chrono::Utc;

use crate::config::Config;
use crate::gates;
use crate::kubectl::KubeCtl;
use crate::nodes;
use crate::output;
use crate::phases;
use crate::types::{MatrixReport, PhaseTimings, Prediction, ScenarioResult};

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
    emitter: &crate::events::EventEmitter,
) -> anyhow::Result<MatrixReport> {
    let timestamp = Utc::now().to_rfc3339();

    emitter.matrix_start(config.scenarios.len());

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

    output::print_phase(&format!("Scaling Matrix: {} scenarios", scenarios.len()));

    // Scale worker node group to desired count (if configured)
    // Errors here don't skip cleanup — we proceed to the scenario loop
    // which will fail on its own, then cleanup runs.
    // Scale observability node group to 1 (monitoring stack)
    if let Some(ong) = &config.observability_node_group {
        output::print_action("Scaling observability node to 1...");
        if let Err(e) = nodes::scale_node_group(ong, 1) {
            output::print_warning(&format!("Observability scaling failed: {e}. Metrics may not be available."));
        }
    }

    if let Some(wng) = &config.worker_node_group {
        output::print_phase("Environment Setup");
        if let Err(e) = nodes::scale_worker_group(wng, wng.desired) {
            output::print_warning(&format!("Worker scaling failed: {e}. Continuing — scenarios may fail."));
        } else {
            output::print_action(&format!(
                "Waiting for {} worker nodes to be ready...",
                wng.desired
            ));
            let _ = nodes::wait_for_nodes(
                kubectl,
                wng.desired,
                std::time::Duration::from_secs(config.timeout_secs),
                std::time::Duration::from_secs(config.node_poll_interval_secs),
            );
        }
    }

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

        let result = run_single_scenario(kubectl, config, scenario, skip_scaling, emitter);
        results.push(result);

        // Inter-scenario cleanup via Phase 1 RESET (skip after last)
        if i < scenarios.len() - 1 {
            output::print_inter_scenario_cleanup();

            // Use Phase 1 (RESET) for inter-scenario cleanup
            if let Err(e) = phases::run_phase_1_reset(kubectl, config) {
                output::print_warning(&format!("Inter-scenario reset failed: {e}"));
            }

            // [Gate 5] Drain Gate -- verify 0 pods AND infrastructure recovery
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

    // Resume HelmReleases, kustomizations, and reset replicas after all scenarios
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

        // Resume kustomizations suspended during warmup
        for ks in &config.suspend_kustomizations {
            let _ = kubectl.run(&[
                "-n", &config.suspend_kustomizations_namespace,
                "patch", "kustomization", ks,
                "--type=merge", "-p", r#"{"spec":{"suspend":false}}"#,
            ]);
        }

        // Resume GW + WH HelmReleases (suspended during scaling).
        // Flux will reconcile back to the git-defined replica count.
        let _ = kubectl.run(&[
            "-n", &config.injection_namespace, "patch",
            "helmrelease.helm.toolkit.fluxcd.io", &config.gateway_release,
            "--type=merge", "-p", r#"{"spec":{"suspend":false}}"#,
        ]);
        let _ = kubectl.run(&[
            "-n", &config.injection_namespace, "patch",
            "helmrelease.helm.toolkit.fluxcd.io", &config.webhook_release,
            "--type=merge", "-p", r#"{"spec":{"suspend":false}}"#,
        ]);
    }

    // Scale burst node group back to 0 — always attempt
    if let Some(ng) = &config.node_group {
        output::print_phase("Scaling Burst Nodes to 0");
        if let Err(e) = nodes::scale_node_group(ng, 0) {
            output::print_warning(&format!("Failed to scale down burst nodes: {e}"));
        }

        // Verified teardown: wait until burst nodes actually reach 0
        if config.verify_teardown {
            nodes::wait_for_zero_burst_nodes(
                kubectl,
                &ng.nodegroup_name,
                std::time::Duration::from_secs(300),
            )?;
        }
    }

    // Restore worker node group to baseline — always attempt
    if let Some(wng) = &config.worker_node_group {
        output::print_phase("Restoring Workers to Baseline");
        if let Err(e) = nodes::scale_worker_group(wng, wng.baseline) {
            output::print_warning(&format!("Failed to restore workers: {e}"));
        }
    }

    // Restore dedicated GW / WH node groups to their baseline — always attempt.
    // This drops the cost back to a single warm node per pool between matrix
    // runs while leaving the pool ready for the next dispatch.
    if let Some(gng) = &config.gateway_node_group {
        output::print_phase("Restoring Gateway Nodes to Baseline");
        if let Err(e) = nodes::scale_infra_node_group(
            kubectl,
            gng,
            gng.baseline,
            "gateway",
            std::time::Duration::from_secs(300),
        ) {
            output::print_warning(&format!("Failed to restore gateway nodes: {e}"));
        }
    }
    if let Some(wng) = &config.webhook_node_group {
        output::print_phase("Restoring Webhook Nodes to Baseline");
        if let Err(e) = nodes::scale_infra_node_group(
            kubectl,
            wng,
            wng.baseline,
            "webhook",
            std::time::Duration::from_secs(300),
        ) {
            output::print_warning(&format!("Failed to restore webhook nodes: {e}"));
        }
    }

    // Scale observability node group back to 0
    if let Some(ong) = &config.observability_node_group {
        output::print_action("Scaling observability node to 0...");
        if let Err(e) = nodes::scale_node_group(ong, 0) {
            output::print_warning(&format!("Failed to scale down observability: {e}"));
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

    let passed = total_count - failure_count;
    emitter.matrix_complete(total_count, passed, failure_count);

    if output::is_json_mode() {
        output::json_emit(&serde_json::json!({
            "type": "matrix_complete",
            "total": total_count,
            "passed": passed,
            "failed": failure_count,
        }));
    }

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
        phase_timings: None,
        error: Some(error),
    }
}

/// Run a single scenario through the three-phase lifecycle and capture the result.
fn run_single_scenario(
    kubectl: &KubeCtl,
    config: &Config,
    scenario: &crate::config::Scenario,
    skip_scaling: bool,
    emitter: &crate::events::EventEmitter,
) -> ScenarioResult {
    // Phase 1: RESET -- get to verified zero state
    let reset_ms = match phases::run_phase_1_reset(kubectl, config) {
        Ok(ms) => {
            emitter.phase_complete(&scenario.name, "RESET", ms);
            ms
        }
        Err(e) => {
            emitter.scenario_complete(&scenario.name, false, Some(&e.to_string()));
            if output::is_json_mode() {
                output::json_emit(&serde_json::json!({
                    "type": "scenario_error",
                    "scenario": scenario.name,
                    "phase": "RESET",
                    "error": e.to_string(),
                }));
            }
            return make_error_result(scenario, format!("Phase 1 RESET failed: {e}"));
        }
    };

    // Phase 2: WARMUP -- infrastructure ready
    let warmup_timings = match phases::run_phase_2_warmup(kubectl, config, scenario, skip_scaling) {
        Ok(t) => {
            emitter.phase_complete(&scenario.name, "WARMUP", t.total_ms);
            t
        }
        Err(e) => {
            emitter.scenario_complete(&scenario.name, false, Some(&e.to_string()));
            if output::is_json_mode() {
                output::json_emit(&serde_json::json!({
                    "type": "scenario_error",
                    "scenario": scenario.name,
                    "phase": "WARMUP",
                    "error": e.to_string(),
                }));
            }
            return make_error_result(scenario, format!("Phase 2 WARMUP failed: {e}"));
        }
    };

    // Phase 3: EXECUTION -- burst bandwidth
    let (burst_result, execution_ms) =
        match phases::run_phase_3_execution(kubectl, config, scenario, emitter) {
            Ok((mut b, ms)) => {
                // Attach scaling formula predictions to the result
                b.prediction = Some(Prediction::calculate(
                    scenario.replicas,
                    config.secrets_per_pod,
                    config.qps,
                    scenario.gateway_replicas,
                    scenario.webhook_replicas,
                ));

                emitter.phase_complete(&scenario.name, "EXECUTION", ms);
                emitter.burst_complete(&scenario.name, &b);
                if output::is_json_mode() {
                    output::json_emit(&serde_json::json!({
                        "type": "burst_complete",
                        "scenario": scenario.name,
                        "result": serde_json::to_value(&b).unwrap_or_default(),
                    }));
                }
                (b, ms)
            }
            Err(e) => {
                emitter.scenario_complete(&scenario.name, false, Some(&e.to_string()));
                if output::is_json_mode() {
                    output::json_emit(&serde_json::json!({
                        "type": "scenario_error",
                        "scenario": scenario.name,
                        "phase": "EXECUTION",
                        "error": e.to_string(),
                    }));
                }
                return ScenarioResult {
                    name: scenario.name.clone(),
                    replicas: scenario.replicas,
                    gateway_replicas: scenario.gateway_replicas,
                    webhook_replicas: scenario.webhook_replicas,
                    verify: None,
                    burst: None,
                    phase_timings: Some(PhaseTimings {
                        reset_ms,
                        warmup_ms: warmup_timings.total_ms,
                        warmup_detail: warmup_timings,
                        execution_ms: 0,
                    }),
                    error: Some(format!("Phase 3 EXECUTION failed: {e}")),
                };
            }
        };

    let timings = PhaseTimings {
        reset_ms,
        warmup_ms: warmup_timings.total_ms,
        warmup_detail: warmup_timings,
        execution_ms,
    };

    // Print full phase timing summary
    phases::print_scenario_timings(&timings);

    emitter.scenario_complete(&scenario.name, true, None);

    let result = ScenarioResult {
        name: scenario.name.clone(),
        replicas: scenario.replicas,
        gateway_replicas: scenario.gateway_replicas,
        webhook_replicas: scenario.webhook_replicas,
        verify: None,
        burst: Some(burst_result),
        phase_timings: Some(timings),
        error: None,
    };

    if output::is_json_mode() {
        output::json_emit(&serde_json::json!({
            "type": "scenario_complete",
            "scenario": scenario.name,
            "success": true,
            "result": serde_json::to_value(&result).unwrap_or_default(),
        }));
    }

    result
}
