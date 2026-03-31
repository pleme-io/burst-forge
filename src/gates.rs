//! Explicit phase gate checks for burst-forge.
//!
//! Every gate must pass before the next phase starts.
//! Gates provide clean boundaries between phases with clear diagnostics.

use std::time::{Duration, Instant};

use crate::config::Config;
use crate::drain;
use crate::kubectl::KubeCtl;
use crate::output;

/// Gate check result — either passes or fails with a message.
#[derive(Debug)]
pub struct GateResult {
    pub gate: &'static str,
    pub passed: bool,
    pub message: String,
    /// Short detail for dot-leader line (e.g., "19/19 Ready+Schedulable").
    pub detail: String,
    /// Expected state (shown on failure).
    pub expected: String,
    /// Actual state (shown on failure).
    pub actual: String,
}

impl GateResult {
    fn pass(gate: &'static str, detail: String, message: String) -> Self {
        Self {
            gate,
            passed: true,
            message,
            detail,
            expected: String::new(),
            actual: String::new(),
        }
    }

    fn fail(gate: &'static str, detail: String, message: String, expected: String, actual: String) -> Self {
        Self {
            gate,
            passed: false,
            message,
            detail,
            expected,
            actual,
        }
    }
}

/// Enforce a gate result: print it and bail if it failed and strict mode is on.
///
/// # Errors
///
/// Returns an error if the gate failed and `strict_gates` is true.
pub fn enforce(result: &GateResult, strict: bool) -> anyhow::Result<()> {
    if result.passed {
        output::print_gate_result(result.gate, &result.detail, true);
        Ok(())
    } else if strict {
        output::print_gate_result(result.gate, &result.detail, false);
        if !result.expected.is_empty() {
            output::print_gate_failure_detail(&result.expected, &result.actual);
        }
        anyhow::bail!(
            "{} FAILED: {}",
            result.gate,
            result.message
        );
    } else {
        output::print_gate_result(result.gate, &result.detail, false);
        if !result.expected.is_empty() {
            output::print_gate_failure_detail(&result.expected, &result.actual);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Gate 1: Node Ready Gate
// ---------------------------------------------------------------------------

/// Count nodes that are both Ready and schedulable (not `SchedulingDisabled`).
///
/// A node showing `Ready,SchedulingDisabled` is draining and must not be counted.
///
/// # Errors
///
/// Returns an error if kubectl fails.
pub fn count_ready_schedulable_nodes(kubectl: &KubeCtl) -> anyhow::Result<u32> {
    let output = kubectl.run(&["get", "nodes", "--no-headers"])?;
    #[allow(clippy::cast_possible_truncation)]
    let count = output
        .lines()
        .filter(|line| {
            line.contains("Ready")
                && !line.contains("NotReady")
                && !line.contains("SchedulingDisabled")
        })
        .count() as u32;
    Ok(count)
}

/// Wait for the desired number of nodes to be Ready AND schedulable.
///
/// Polls at the given interval until the count is met or the timeout expires.
///
/// # Errors
///
/// Returns an error if the timeout is exceeded.
pub fn wait_for_ready_schedulable_nodes(
    kubectl: &KubeCtl,
    desired: u32,
    timeout: Duration,
    poll_interval: Duration,
) -> anyhow::Result<GateResult> {
    output::print_status(&format!(
        "Waiting for {desired} Ready+Schedulable nodes (timeout: {}s)...",
        timeout.as_secs()
    ));

    let start = Instant::now();

    loop {
        let ready = count_ready_schedulable_nodes(kubectl)?;
        let elapsed = start.elapsed().as_secs();

        if ready >= desired {
            return Ok(GateResult::pass(
                "[Gate 1]",
                format!("Node Ready {}", output::dim(&format!("{ready}/{desired}"))),
                format!("Nodes: {ready}/{desired} Ready+Schedulable"),
            ));
        }

        output::print_progress(elapsed, &format!("Ready+Schedulable nodes: {ready}/{desired}"));

        if start.elapsed() > timeout {
            return Ok(GateResult::fail(
                "[Gate 1]",
                format!("Node Ready {}", output::dim(&format!("{ready}/{desired}"))),
                format!("Nodes: {ready}/{desired} Ready+Schedulable after {}s timeout", timeout.as_secs()),
                format!("{desired}/{desired} Ready+Schedulable"),
                format!("{ready}/{desired} Ready+Schedulable after {}s", timeout.as_secs()),
            ));
        }

        std::thread::sleep(poll_interval);
    }
}

// ---------------------------------------------------------------------------
// Gate 2: Warmup Gate
// ---------------------------------------------------------------------------

/// Verify `DaemonSet` readiness matches the count of Ready+schedulable nodes.
///
/// The `DaemonSet` desired count should equal the number of schedulable nodes,
/// and all desired pods must be ready.
///
/// # Errors
///
/// Returns an error if kubectl fails.
pub fn check_warmup_gate(
    kubectl: &KubeCtl,
    namespace: &str,
    name: &str,
    timeout: Duration,
) -> anyhow::Result<GateResult> {
    let schedulable = count_ready_schedulable_nodes(kubectl)?;

    output::print_status(&format!(
        "Warmup: expecting DaemonSet {namespace}/{name} to match {schedulable} schedulable nodes"
    ));

    let start = Instant::now();
    let poll_interval = Duration::from_secs(15);

    loop {
        let result = kubectl.get_json(&["-n", namespace, "get", "daemonset", name]);

        match result {
            Ok(json) => {
                let desired = json["status"]["desiredNumberScheduled"]
                    .as_u64()
                    .unwrap_or(0);
                let ready = json["status"]["numberReady"].as_u64().unwrap_or(0);
                let elapsed = start.elapsed().as_secs();

                output::print_progress(elapsed, &format!(
                    "DaemonSet {name}: {ready}/{desired} ready, {schedulable} schedulable nodes"
                ));

                #[allow(clippy::cast_possible_truncation)]
                if ready >= u64::from(schedulable) && desired >= u64::from(schedulable) && schedulable > 0 {
                    return Ok(GateResult::pass(
                        "[Gate 2]",
                        format!("Warmup {}", output::dim(&format!("{ready}/{desired}"))),
                        format!("Warmup: {ready}/{desired} pods on {schedulable} schedulable nodes"),
                    ));
                }
            }
            Err(e) => {
                let elapsed = start.elapsed().as_secs();
                output::print_progress(elapsed, &format!("DaemonSet {name}: not found yet ({e})"));
            }
        }

        if start.elapsed() > timeout {
            return Ok(GateResult::fail(
                "[Gate 2]",
                "Warmup".to_string(),
                format!("Warmup DaemonSet {namespace}/{name} not ready after {}s", timeout.as_secs()),
                format!("{schedulable}/{schedulable} ready on {schedulable} nodes"),
                format!("Not ready after {}s", timeout.as_secs()),
            ));
        }

        std::thread::sleep(poll_interval);
    }
}

// ---------------------------------------------------------------------------
// Gate 3: Infrastructure Gate
// ---------------------------------------------------------------------------

/// Verify gateway and webhook deployments have `readyReplicas == spec.replicas`.
///
/// Waits up to `rollout_wait_secs` for the condition to be met. If not met,
/// returns a failure with details.
///
/// # Errors
///
/// Returns an error if kubectl fails.
pub fn check_infrastructure_gate(
    kubectl: &KubeCtl,
    config: &Config,
    expected_gw: u32,
    expected_wh: u32,
) -> anyhow::Result<GateResult> {
    let timeout = Duration::from_secs(config.rollout_wait_secs);
    let poll_interval = Duration::from_secs(5);
    let start = Instant::now();

    loop {
        let (gw_ready, gw_desired) = drain::get_deployment_replicas(
            kubectl,
            &config.injection_namespace,
            &config.gateway_deployment,
        )?;
        let (wh_ready, wh_desired) = drain::get_deployment_replicas(
            kubectl,
            &config.injection_namespace,
            &config.webhook_deployment,
        )?;

        if gw_ready == expected_gw
            && gw_desired == expected_gw
            && wh_ready == expected_wh
            && wh_desired == expected_wh
        {
            return Ok(GateResult::pass(
                "[Gate 3]",
                format!(
                    "Infrastructure {} GW {gw_ready}/{expected_gw} WH {wh_ready}/{expected_wh}",
                    output::dim(".."),
                ),
                format!("Infrastructure: GW {gw_ready}/{expected_gw}, WH {wh_ready}/{expected_wh}"),
            ));
        }

        if start.elapsed() > timeout {
            return Ok(GateResult::fail(
                "[Gate 3]",
                format!(
                    "Infrastructure {} GW {gw_ready}/{expected_gw} WH {wh_ready}/{expected_wh}",
                    output::dim(".."),
                ),
                format!("Infrastructure not ready after {}s", config.rollout_wait_secs),
                format!("GW {expected_gw}/{expected_gw} ready, WH {expected_wh}/{expected_wh} ready"),
                format!("GW {gw_ready}/{gw_desired} ready, WH {wh_ready}/{wh_desired} ready after {}s", config.rollout_wait_secs),
            ));
        }

        let elapsed = start.elapsed().as_secs();
        output::print_progress(elapsed, &format!(
            "Infra: GW {gw_ready}/{expected_gw}, WH {wh_ready}/{expected_wh} -- waiting..."
        ));
        std::thread::sleep(poll_interval);
    }
}

// ---------------------------------------------------------------------------
// Gate 4: Starting Line Gate
// ---------------------------------------------------------------------------

/// Verify the pre-burst starting line is clean:
/// - Deployment at 0/0
/// - No pods with the burst label (including no Terminating pods)
/// - No pods in non-Succeeded phase matching the label
/// - Gateway and webhook fully ready
///
/// # Errors
///
/// Returns an error if kubectl fails.
pub fn check_starting_line_gate(
    kubectl: &KubeCtl,
    config: &Config,
    expected_gw: u32,
    expected_wh: u32,
) -> anyhow::Result<GateResult> {
    let app_label = config.resolved_pod_label();

    // Check deployment shows 0/0
    let (ready, desired) =
        drain::get_deployment_replicas(kubectl, &config.namespace, &config.deployment)?;
    if ready != 0 || desired != 0 {
        return Ok(GateResult::fail(
            "[Gate 4]",
            "Starting Line".to_string(),
            format!("Starting line: deployment at {ready}/{desired}"),
            "deployment 0/0".to_string(),
            format!("deployment {ready}/{desired}"),
        ));
    }

    // Check no pods exist with label (catches Terminating pods too)
    let pod_count = drain::count_pods(kubectl, &config.namespace, &app_label)?;
    if pod_count != 0 {
        return Ok(GateResult::fail(
            "[Gate 4]",
            "Starting Line".to_string(),
            format!("Starting line: {pod_count} pods still exist with label {app_label}"),
            "0 pods".to_string(),
            format!("{pod_count} pods with label {app_label}"),
        ));
    }

    // Check no pods in non-Succeeded phase via field-selector
    let non_succeeded = kubectl.run(&[
        "-n",
        &config.namespace,
        "get",
        "pods",
        "-l",
        &app_label,
        "--field-selector=status.phase!=Succeeded",
        "--no-headers",
    ]);
    match non_succeeded {
        Ok(kubectl_output) => {
            let lines: Vec<&str> = kubectl_output.lines().filter(|l| !l.trim().is_empty()).collect();
            if !lines.is_empty() {
                return Ok(GateResult::fail(
                    "[Gate 4]",
                    "Starting Line".to_string(),
                    format!("Starting line: {} non-Succeeded pods found", lines.len()),
                    "0 non-Succeeded pods".to_string(),
                    format!("{} non-Succeeded pods", lines.len()),
                ));
            }
        }
        Err(e) => {
            let msg = e.to_string();
            if !msg.contains("No resources found") && !msg.contains("not found") {
                return Err(e);
            }
        }
    }

    // Check gateway and webhook readiness
    let (gw_ready, _) = drain::get_deployment_replicas(
        kubectl,
        &config.injection_namespace,
        &config.gateway_deployment,
    )?;
    let (wh_ready, _) = drain::get_deployment_replicas(
        kubectl,
        &config.injection_namespace,
        &config.webhook_deployment,
    )?;

    if gw_ready < expected_gw {
        return Ok(GateResult::fail(
            "[Gate 4]",
            "Starting Line".to_string(),
            format!("Starting line: gateway {gw_ready}/{expected_gw} -- not ready"),
            format!("GW {expected_gw}/{expected_gw} ready"),
            format!("GW {gw_ready}/{expected_gw} ready"),
        ));
    }
    if wh_ready < expected_wh {
        return Ok(GateResult::fail(
            "[Gate 4]",
            "Starting Line".to_string(),
            format!("Starting line: webhook {wh_ready}/{expected_wh} -- not ready"),
            format!("WH {expected_wh}/{expected_wh} ready"),
            format!("WH {wh_ready}/{expected_wh} ready"),
        ));
    }

    Ok(GateResult::pass(
        "[Gate 4]",
        format!("Starting Line {} 0 pods, clean", output::dim("..")),
        format!("Starting line: 0 pods, deployment 0/0, GW {gw_ready}/{expected_gw} ready, WH {wh_ready}/{expected_wh} ready"),
    ))
}

// ---------------------------------------------------------------------------
// Gate 5: Drain Gate
// ---------------------------------------------------------------------------

/// Verify drain is complete and infrastructure has recovered:
/// - 0 pods with burst label
/// - Gateway `readyReplicas == desired`
/// - Webhook `readyReplicas == desired`
///
/// # Errors
///
/// Returns an error if kubectl fails.
pub fn check_drain_gate(
    kubectl: &KubeCtl,
    config: &Config,
    expected_gw: u32,
    expected_wh: u32,
) -> anyhow::Result<GateResult> {
    let app_label = config.resolved_pod_label();

    // Confirm 0 pods
    let pod_count = drain::count_pods(kubectl, &config.namespace, &app_label)?;
    if pod_count != 0 {
        return Ok(GateResult::fail(
            "[Gate 5]",
            "Drain".to_string(),
            format!("Drain incomplete: {pod_count} pods remaining"),
            "0 pods remaining".to_string(),
            format!("{pod_count} pods remaining"),
        ));
    }

    // Wait for gateway/webhook to recover from load
    let timeout = Duration::from_secs(config.rollout_wait_secs);
    let poll_interval = Duration::from_secs(5);
    let start = Instant::now();

    loop {
        let (gw_ready, gw_desired) = drain::get_deployment_replicas(
            kubectl,
            &config.injection_namespace,
            &config.gateway_deployment,
        )?;
        let (wh_ready, wh_desired) = drain::get_deployment_replicas(
            kubectl,
            &config.injection_namespace,
            &config.webhook_deployment,
        )?;

        if gw_ready == expected_gw
            && gw_desired == expected_gw
            && wh_ready == expected_wh
            && wh_desired == expected_wh
        {
            return Ok(GateResult::pass(
                "[Gate 5]",
                format!(
                    "Drain {} 0 pods, GW healthy",
                    output::dim(".."),
                ),
                format!("Drain complete: 0 pods, GW {gw_ready}/{expected_gw} healthy, WH {wh_ready}/{expected_wh} healthy"),
            ));
        }

        if start.elapsed() > timeout {
            return Ok(GateResult::fail(
                "[Gate 5]",
                "Drain".to_string(),
                format!("Drain complete but infrastructure not recovered after {}s", config.rollout_wait_secs),
                format!("GW {expected_gw}/{expected_gw}, WH {expected_wh}/{expected_wh}"),
                format!("GW {gw_ready}/{gw_desired}, WH {wh_ready}/{wh_desired} after {}s", config.rollout_wait_secs),
            ));
        }

        let elapsed = start.elapsed().as_secs();
        output::print_progress(elapsed, &format!(
            "Drain: 0 pods, GW {gw_ready}/{expected_gw}, WH {wh_ready}/{expected_wh} -- recovery..."
        ));
        std::thread::sleep(poll_interval);
    }
}
