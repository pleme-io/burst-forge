//! Three-phase burst test lifecycle: RESET, WARMUP, EXECUTION.
//!
//! Wraps existing gates, drain, burst logic in a timing framework that makes
//! each phase's cost visible without changing the underlying logic.

use std::time::{Duration, Instant};

use crate::burst;
use crate::config::{Config, Scenario};
use crate::drain;
use crate::gates;
use crate::kubectl::KubeCtl;
use crate::output;
use crate::types::{BurstResult, PhaseTimings, WarmupTimings};
use crate::verify;

/// Run Phase 1: RESET -- get to verified zero state as fast as possible.
///
/// Returns elapsed milliseconds.
///
/// # Errors
///
/// Returns an error if draining or verification fails.
pub fn run_phase_1_reset(
    kubectl: &KubeCtl,
    config: &Config,
) -> anyhow::Result<u64> {
    output::print_phase("Phase 1: RESET");
    let start = Instant::now();

    let app_label = config.resolved_pod_label();

    // Scale deployment to 0
    output::print_action("Scaling deployment to 0...");
    let _ = kubectl.run(&[
        "-n", &config.namespace, "scale", "deployment",
        &config.deployment, "--replicas=0",
    ]);

    // Force delete or graceful drain based on config
    if config.reset.force_delete {
        output::print_action("Force deleting pods (--grace-period=0 --force)...");
        drain::force_delete_pods(kubectl, &config.namespace, &app_label)?;
        // Brief wait for force-delete to take effect
        std::thread::sleep(Duration::from_secs(2));
    } else {
        // Wait for graceful termination, force-delete if grace period exceeded
        let grace_start = Instant::now();
        let grace_timeout = Duration::from_secs(config.reset.grace_period_secs);

        loop {
            let count = drain::count_pods(kubectl, &config.namespace, &app_label)?;
            if count == 0 {
                break;
            }

            if grace_start.elapsed() > grace_timeout {
                output::print_action("Grace period exceeded, force deleting...");
                drain::force_delete_pods(kubectl, &config.namespace, &app_label)?;
                std::thread::sleep(Duration::from_secs(2));
                break;
            }

            #[allow(clippy::cast_possible_truncation)]
            let elapsed = grace_start.elapsed().as_secs();
            output::print_progress(elapsed, &format!("Draining: {count} pods remaining"));
            std::thread::sleep(Duration::from_secs(config.drain_poll_interval_secs));
        }
    }

    // Verify zero pods
    let remaining = drain::count_pods(kubectl, &config.namespace, &app_label)?;
    if remaining > 0 {
        output::print_warning(&format!("{remaining} pods still remaining after reset"));
    }

    #[allow(clippy::cast_possible_truncation)]
    let elapsed_ms = start.elapsed().as_millis() as u64;
    output::print_phase_timing("Phase 1: RESET", elapsed_ms);

    Ok(elapsed_ms)
}

/// Run Phase 2: WARMUP -- get all infrastructure to verified ready state.
///
/// Sub-phases:
/// - 2a: Node scaling (if node_group configured)
/// - 2b: Image warmup (if warmup_daemonset configured)
/// - 2c: Injection infrastructure (gateway + webhook scaled and READY)
/// - 2d: Gate verification (all gates pass)
///
/// # Errors
///
/// Returns an error if any warmup sub-phase fails fatally.
#[allow(clippy::too_many_lines)]
pub fn run_phase_2_warmup(
    kubectl: &KubeCtl,
    config: &Config,
    scenario: &Scenario,
    skip_scaling: bool,
) -> anyhow::Result<WarmupTimings> {
    output::print_phase("Phase 2: WARMUP");
    let phase_start = Instant::now();
    let mut timings = WarmupTimings::default();

    // 2a: Node scaling
    let sub_start = Instant::now();
    if let Some(ng) = &config.node_group {
        let nodes_needed = scenario.nodes.unwrap_or_else(|| {
            crate::nodes::calculate_nodes(scenario.replicas, ng.pods_per_node)
                .min(ng.max_nodes)
        });

        output::print_action(&format!("2a. Scaling nodes to {nodes_needed}..."));
        crate::nodes::scale_node_group(ng, nodes_needed)?;

        let gate1 = gates::wait_for_ready_schedulable_nodes(
            kubectl,
            nodes_needed,
            Duration::from_secs(config.timeout_secs),
            Duration::from_secs(config.node_poll_interval_secs),
        )?;
        gates::enforce(&gate1, config.strict_gates)?;

        crate::nodes::tag_nodes(kubectl, "burst-forge=true")?;
    }
    #[allow(clippy::cast_possible_truncation)]
    {
        timings.nodes_ms = sub_start.elapsed().as_millis() as u64;
    }

    // 2b: Image warmup (DaemonSet)
    let sub_start = Instant::now();
    if let Some(warmup) = &config.warmup_daemonset {
        output::print_action(&format!(
            "2b. Warming images via DaemonSet {}/{}...",
            warmup.namespace, warmup.name
        ));
        let gate2 = gates::check_warmup_gate(
            kubectl,
            &warmup.namespace,
            &warmup.name,
            Duration::from_secs(warmup.timeout_secs),
        )?;
        gates::enforce(&gate2, config.strict_gates)?;
    }
    #[allow(clippy::cast_possible_truncation)]
    {
        timings.images_ms = sub_start.elapsed().as_millis() as u64;
    }

    // 2b+: IPAMD warmup — wait for secondary ENI attachment on burst nodes.
    // Custom networking requires 2-3 minutes after node Ready for IPAMD
    // to attach ENIs on /20 pod subnets and allocate prefix warm pool.
    let sub_start = Instant::now();
    if config.ipamd_warmup_secs > 0 {
        output::print_action(&format!(
            "2b+. Waiting {}s for IPAMD ENI warmup...",
            config.ipamd_warmup_secs
        ));
        let warmup_duration = Duration::from_secs(config.ipamd_warmup_secs);
        let poll = Duration::from_secs(15);
        let start = Instant::now();
        while start.elapsed() < warmup_duration {
            #[allow(clippy::cast_possible_truncation)]
            let elapsed = start.elapsed().as_secs();
            output::print_progress(elapsed, &format!(
                "IPAMD warmup: {}s / {}s",
                elapsed, config.ipamd_warmup_secs
            ));
            std::thread::sleep(poll.min(warmup_duration.saturating_sub(start.elapsed())));
        }
        output::print_action("IPAMD warmup complete");
    }
    #[allow(clippy::cast_possible_truncation)]
    {
        timings.ipamd_warmup_ms = sub_start.elapsed().as_millis() as u64;
    }

    // 2c: Gateway scaling
    let sub_start = Instant::now();
    if !skip_scaling {
        // Suspend kustomizations (prevents GitOps from reverting deployment replicas)
        for ks in &config.suspend_kustomizations {
            output::print_action(&format!("  Suspending kustomization {ks}..."));
            let _ = kubectl.run(&[
                "-n", &config.suspend_kustomizations_namespace,
                "patch", "kustomization", ks,
                "--type=merge", "-p", r#"{"spec":{"suspend":true}}"#,
            ]);
        }

        // Suspend HelmReleases
        output::print_action("2c. Scaling injection infrastructure...");
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

        // Scale gateway
        output::print_action(&format!(
            "  Gateway -> {} replicas...", scenario.gateway_replicas
        ));
        kubectl.run(&[
            "-n", &config.injection_namespace, "scale", "deployment",
            &config.gateway_deployment,
            &format!("--replicas={}", scenario.gateway_replicas),
        ])?;

        // Wait for gateway rollout
        let gw_deploy_path = format!("deployment/{}", config.gateway_deployment);
        let _ = kubectl.run(&[
            "-n", &config.injection_namespace, "rollout", "status",
            &gw_deploy_path,
            &format!("--timeout={}s", config.rollout_wait_secs),
        ]);
    }
    #[allow(clippy::cast_possible_truncation)]
    {
        timings.gateway_ms = sub_start.elapsed().as_millis() as u64;
    }

    // 2d: Webhook scaling
    let sub_start = Instant::now();
    if !skip_scaling {
        output::print_action(&format!(
            "  Webhook -> {} replicas...", scenario.webhook_replicas
        ));
        kubectl.run(&[
            "-n", &config.injection_namespace, "scale", "deployment",
            &config.webhook_deployment,
            &format!("--replicas={}", scenario.webhook_replicas),
        ])?;

        // Wait for webhook rollout
        let wh_deploy_path = format!("deployment/{}", config.webhook_deployment);
        let _ = kubectl.run(&[
            "-n", &config.injection_namespace, "rollout", "status",
            &wh_deploy_path,
            &format!("--timeout={}s", config.rollout_wait_secs),
        ]);
    }
    #[allow(clippy::cast_possible_truncation)]
    {
        timings.webhook_ms = sub_start.elapsed().as_millis() as u64;
    }

    // 2e: Gate verification
    let sub_start = Instant::now();
    if !skip_scaling {
        // [Gate 3] Infrastructure gate
        output::print_action("2e. Verifying gates...");
        let gate3 = gates::check_infrastructure_gate(
            kubectl,
            config,
            scenario.gateway_replicas,
            scenario.webhook_replicas,
        )?;
        gates::enforce(&gate3, config.strict_gates)?;
    }

    // Verify infrastructure
    verify::verify_infra(kubectl, config)?;

    // [Gate 4] Starting line gate
    let gate4 = gates::check_starting_line_gate(
        kubectl,
        config,
        scenario.gateway_replicas,
        scenario.webhook_replicas,
    )?;
    gates::enforce(&gate4, config.strict_gates)?;
    #[allow(clippy::cast_possible_truncation)]
    {
        timings.gates_ms = sub_start.elapsed().as_millis() as u64;
    }

    // 2f: Per-scenario pod spec patches (init latency, memory)
    let sub_start = Instant::now();
    if scenario.init_sleep_secs.is_some() || scenario.pod_memory_request.is_some() {
        burst::apply_scenario_patches(kubectl, config, scenario)?;
    }
    #[allow(clippy::cast_possible_truncation)]
    {
        timings.patches_ms = sub_start.elapsed().as_millis() as u64;
    }

    #[allow(clippy::cast_possible_truncation)]
    {
        timings.total_ms = phase_start.elapsed().as_millis() as u64;
    }

    output::print_warmup_summary(&timings);

    Ok(timings)
}

/// Run Phase 3: EXECUTION -- maximum pod injection rate.
///
/// Returns the burst result and elapsed milliseconds.
///
/// # Errors
///
/// Returns an error if the burst test fails.
pub fn run_phase_3_execution(
    kubectl: &KubeCtl,
    config: &Config,
    scenario: &Scenario,
) -> anyhow::Result<(BurstResult, u64)> {
    output::print_phase("Phase 3: EXECUTION");
    let start = Instant::now();

    let result = burst::run_burst(
        kubectl,
        config,
        scenario.replicas,
        1,
        scenario.gateway_replicas,
        scenario.webhook_replicas,
    )?;

    #[allow(clippy::cast_possible_truncation)]
    let elapsed_ms = start.elapsed().as_millis() as u64;

    output::print_execution_summary(&result, elapsed_ms, scenario.gateway_replicas);

    Ok((result, elapsed_ms))
}

/// Print the full phase timing summary for a completed scenario.
pub fn print_scenario_timings(timings: &PhaseTimings) {
    output::print_phase("Phase Timings");
    output::print_phase_breakdown(timings);
}
