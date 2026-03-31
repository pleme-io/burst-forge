//! Burst test execution — scale 0 -> N and measure.

use std::time::{Duration, Instant};

use chrono::Utc;

use crate::config::Config;
use crate::drain;
use crate::kubectl::KubeCtl;
use crate::output;
use crate::types::BurstResult;

/// Run a single burst test: drain to 0 (verified), then scale to N, poll until done.
///
/// The drain phase polls `kubectl get pods` until truly 0 pods exist,
/// rather than trusting `kubectl scale` alone. If drain times out,
/// force-deletes remaining pods before proceeding.
///
/// # Errors
///
/// Returns an error if kubectl commands fail.
#[allow(clippy::too_many_lines)]
pub fn run_burst(
    kubectl: &KubeCtl,
    config: &Config,
    replicas: u32,
    iteration: u32,
    expected_gw: u32,
    expected_wh: u32,
) -> anyhow::Result<BurstResult> {
    let start = Instant::now();
    let timestamp = Utc::now().to_rfc3339();

    output::print_phase(&format!("Burst #{iteration}: 0 -> {replicas} replicas"));

    // Patch maxSurge to match replica count — all pods created simultaneously
    // for maximum concurrent pressure on gateway/webhook
    output::print_action(&format!("Patching maxSurge={replicas} for simultaneous pod creation..."));
    kubectl.run(&[
        "-n",
        &config.namespace,
        "patch",
        "deployment",
        &config.deployment,
        "--type=merge",
        &format!("-p={{\"spec\":{{\"strategy\":{{\"rollingUpdate\":{{\"maxSurge\":{replicas}}}}}}}}}"),
    ])?;

    // Drain to 0 with verified polling (not just scale + sleep)
    output::print_action("Draining to verified 0 pods...");
    drain::drain_pods(kubectl, config)?;

    // Pre-burst starting line verification
    output::print_action("Verifying starting line...");
    drain::verify_starting_line(kubectl, config, expected_gw, expected_wh)?;

    // BURST: Scale to N
    let burst_start = Instant::now();
    output::print_burst_start(replicas);
    kubectl.run(&[
        "-n",
        &config.namespace,
        "scale",
        "deployment",
        &config.deployment,
        &format!("--replicas={replicas}"),
    ])?;

    // Poll pod status until timeout
    let mut first_ready_time: Option<u64> = None;
    let mut full_admission_time: Option<u64> = None;
    let mut half_running_time: Option<u64> = None;
    let poll_interval = Duration::from_secs(config.poll_interval_secs);
    let timeout_duration = Duration::from_secs(config.timeout_secs);
    let app_label = config.resolved_pod_label();
    let half_target = replicas / 2;

    loop {
        if burst_start.elapsed() > timeout_duration {
            output::print_timeout(config.timeout_secs);
            break;
        }

        let pods = kubectl.get_json(&[
            "-n",
            &config.namespace,
            "get",
            "pods",
            "-l",
            &app_label,
        ])?;

        let empty = vec![];
        let items = pods["items"].as_array().unwrap_or(&empty);

        let (running, pending, failed, injected) =
            count_pod_states(items, config);

        #[allow(clippy::cast_possible_truncation)]
        let elapsed_ms = burst_start.elapsed().as_millis() as u64;

        if running > 0 && first_ready_time.is_none() {
            first_ready_time = Some(elapsed_ms);
        }

        if running >= half_target && half_running_time.is_none() {
            half_running_time = Some(elapsed_ms);
        }

        if injected >= replicas && full_admission_time.is_none() {
            full_admission_time = Some(elapsed_ms);
        }

        output::print_progress_ms(
            elapsed_ms,
            &format!(
                "Running: {running:>4} | Pending: {pending:>4} | Failed: {failed:>3} | Injected: {injected:>4}"
            ),
        );

        if running >= replicas {
            let rate = injection_rate(running, injected);
            output::print_burst_complete(running, replicas, elapsed_ms, rate);
            #[allow(clippy::cast_precision_loss)]
            let admission_rate = if elapsed_ms > 0 {
                f64::from(injected) / (elapsed_ms as f64 / 1000.0)
            } else {
                0.0
            };
            #[allow(clippy::cast_precision_loss)]
            let gw_throughput = if elapsed_ms > 0 {
                f64::from(running) / (elapsed_ms as f64 / 1000.0)
            } else {
                0.0
            };
            return Ok(BurstResult {
                timestamp,
                replicas_requested: replicas,
                pods_running: running,
                pods_failed: failed,
                pods_pending: pending,
                pods_injected: injected,
                injection_success_rate: rate,
                time_to_first_ready_ms: first_ready_time.unwrap_or(0),
                time_to_all_ready_ms: Some(elapsed_ms),
                time_to_full_admission_ms: full_admission_time,
                time_to_50pct_running_ms: half_running_time,
                admission_rate_pods_per_sec: admission_rate,
                gateway_throughput_pods_per_sec: gw_throughput,
                #[allow(clippy::cast_possible_truncation)]
                duration_ms: start.elapsed().as_millis() as u64,
                #[allow(clippy::cast_possible_truncation)]
                nodes: items.len() as u32,
                iteration,
            });
        }

        if running > 0 && pending == 0 && failed == 0 && running < replicas {
            output::print_capacity_limit(running, replicas);
            break;
        }

        std::thread::sleep(poll_interval);
    }

    // Final count
    let pods = kubectl.get_json(&[
        "-n",
        &config.namespace,
        "get",
        "pods",
        "-l",
        &app_label,
    ])?;
    let empty = vec![];
    let items = pods["items"].as_array().unwrap_or(&empty);
    let (running, pending, failed, injected) =
        count_pod_states(items, config);

    #[allow(clippy::cast_possible_truncation)]
    let final_elapsed_ms = burst_start.elapsed().as_millis() as u64;
    #[allow(clippy::cast_precision_loss)]
    let admission_rate = if final_elapsed_ms > 0 {
        f64::from(injected) / (final_elapsed_ms as f64 / 1000.0)
    } else {
        0.0
    };
    #[allow(clippy::cast_precision_loss)]
    let gw_throughput = if final_elapsed_ms > 0 {
        f64::from(running) / (final_elapsed_ms as f64 / 1000.0)
    } else {
        0.0
    };

    Ok(BurstResult {
        timestamp,
        replicas_requested: replicas,
        pods_running: running,
        pods_failed: failed,
        pods_pending: pending,
        pods_injected: injected,
        injection_success_rate: injection_rate(running, injected),
        time_to_first_ready_ms: first_ready_time.unwrap_or(0),
        time_to_all_ready_ms: None,
        time_to_full_admission_ms: full_admission_time,
        time_to_50pct_running_ms: half_running_time,
        admission_rate_pods_per_sec: admission_rate,
        gateway_throughput_pods_per_sec: gw_throughput,
        #[allow(clippy::cast_possible_truncation)]
        duration_ms: start.elapsed().as_millis() as u64,
        nodes: 0,
        iteration,
    })
}

/// Count pods by phase and injection presence.
///
/// Detection strategy depends on `injection_mode`:
/// - **Sidecar:** 2+ containers means a sidecar was injected.
/// - **Env:** any container has an env var matching `injection_env_prefix`.
fn count_pod_states(
    items: &[serde_json::Value],
    config: &Config,
) -> (u32, u32, u32, u32) {
    let mut running = 0u32;
    let mut pending = 0u32;
    let mut failed = 0u32;
    let mut injected = 0u32;

    for pod in items {
        let phase = pod["status"]["phase"].as_str().unwrap_or("");

        match phase {
            "Running" => running += 1,
            "Pending" => pending += 1,
            "Failed" => failed += 1,
            _ => {}
        }

        if has_injection(pod, config) {
            injected += 1;
        }
    }

    (running, pending, failed, injected)
}

/// Check whether a single pod shows evidence of secret injection.
fn has_injection(pod: &serde_json::Value, config: &Config) -> bool {
    use crate::config::InjectionMode;

    let containers = pod["spec"]["containers"].as_array();

    match &config.injection_mode {
        InjectionMode::Sidecar => {
            // 2+ containers indicates sidecar injection
            containers.is_some_and(|c| c.len() >= 2)
        }
        InjectionMode::Env => {
            // Check if any container has env vars matching the configured prefix
            let Some(containers) = containers else {
                return false;
            };
            let prefix = &config.injection_env_prefix;
            containers.iter().any(|c| {
                c["env"].as_array().is_some_and(|envs| {
                    envs.iter().any(|e| {
                        e["name"]
                            .as_str()
                            .is_some_and(|n| n.starts_with(prefix))
                    })
                })
            })
        }
    }
}

/// Calculate the injection success rate as a percentage.
fn injection_rate(running: u32, injected: u32) -> f64 {
    if running > 0 {
        f64::from(injected) / f64::from(running) * 100.0
    } else {
        0.0
    }
}
