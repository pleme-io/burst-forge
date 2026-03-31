//! Burst test execution — scale 0 -> N and measure.

use std::time::{Duration, Instant};

use chrono::Utc;

use crate::config::{Config, InjectionMode};
use crate::kubectl::KubeCtl;
use crate::types::BurstResult;

/// Run a single burst test: scale to 0, then scale to N, poll until done.
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
) -> anyhow::Result<BurstResult> {
    let start = Instant::now();
    let timestamp = Utc::now().to_rfc3339();

    println!("\n=== Burst #{iteration}: 0 -> {replicas} replicas ===\n");

    // Scale to 0 first (clean state)
    println!("  Resetting to 0...");
    kubectl.run(&[
        "-n",
        &config.namespace,
        "scale",
        "deployment",
        &config.deployment,
        "--replicas=0",
    ])?;

    // Wait for scale-down
    std::thread::sleep(Duration::from_secs(5));

    // BURST: Scale to N
    let burst_start = Instant::now();
    println!("  BURST: scaling to {replicas}...");
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
    let poll_interval = Duration::from_secs(config.poll_interval_secs);
    let timeout_duration = Duration::from_secs(config.timeout_secs);
    let app_label = config.resolved_pod_label();

    loop {
        if burst_start.elapsed() > timeout_duration {
            println!("  TIMEOUT after {}s", config.timeout_secs);
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
            count_pod_states(items, &config.injection_mode);

        #[allow(clippy::cast_possible_truncation)]
        let elapsed_ms = burst_start.elapsed().as_millis() as u64;

        if running > 0 && first_ready_time.is_none() {
            first_ready_time = Some(elapsed_ms);
        }

        print!(
            "\r  [{elapsed_ms:>5}ms] Running: {running:>3} | Pending: {pending:>3} | Failed: {failed:>3} | Injected: {injected:>3}"
        );

        if running >= replicas {
            println!("\n  ALL {replicas} PODS READY in {elapsed_ms}ms");
            return Ok(BurstResult {
                timestamp,
                replicas_requested: replicas,
                pods_running: running,
                pods_failed: failed,
                pods_pending: pending,
                pods_injected: injected,
                injection_success_rate: injection_rate(running, injected),
                time_to_first_ready_ms: first_ready_time.unwrap_or(0),
                time_to_all_ready_ms: Some(elapsed_ms),
                #[allow(clippy::cast_possible_truncation)]
                duration_ms: start.elapsed().as_millis() as u64,
                #[allow(clippy::cast_possible_truncation)]
                nodes: items.len() as u32,
                iteration,
            });
        }

        if running > 0 && pending == 0 && failed == 0 && running < replicas {
            println!(
                "\n  CAPACITY LIMIT: {running}/{replicas} pods (no more schedulable)"
            );
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
        count_pod_states(items, &config.injection_mode);

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
        #[allow(clippy::cast_possible_truncation)]
        duration_ms: start.elapsed().as_millis() as u64,
        nodes: 0,
        iteration,
    })
}

/// Count pods by phase and injection presence.
///
/// Detection strategy depends on `mode`:
/// - **Sidecar:** 2+ containers means the Akeyless sidecar was injected.
/// - **Env:** any container has an `AKEYLESS_`-prefixed environment variable.
fn count_pod_states(
    items: &[serde_json::Value],
    mode: &InjectionMode,
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

        if has_injection(pod, mode) {
            injected += 1;
        }
    }

    (running, pending, failed, injected)
}

/// Check whether a single pod shows evidence of Akeyless injection.
fn has_injection(pod: &serde_json::Value, mode: &InjectionMode) -> bool {
    let containers = pod["spec"]["containers"].as_array();

    match mode {
        InjectionMode::Sidecar => {
            // 2+ containers indicates Akeyless sidecar injection
            containers.is_some_and(|c| c.len() >= 2)
        }
        InjectionMode::Env => {
            // Check if any container has AKEYLESS_-prefixed env vars
            let Some(containers) = containers else {
                return false;
            };
            containers.iter().any(|c| {
                c["env"].as_array().is_some_and(|envs| {
                    envs.iter().any(|e| {
                        e["name"]
                            .as_str()
                            .is_some_and(|n| n.starts_with("AKEYLESS_"))
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
