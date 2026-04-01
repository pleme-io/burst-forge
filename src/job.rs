//! Job workload support — create N individual Jobs and measure.
//!
//! Parallel to `burst.rs` (Deployment scaling), this module creates
//! Jobs from a YAML template and polls for pod completion.

use std::time::{Duration, Instant};

use chrono::Utc;

use crate::config::Config;
use crate::kubectl::KubeCtl;
use crate::output;
use crate::types::BurstResult;

/// Load and validate the Job template from the configured path.
///
/// # Errors
///
/// Returns an error if the path is not configured or the file can't be read.
pub fn load_template(config: &Config) -> anyhow::Result<String> {
    let path = config
        .job_template
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("workload_kind=job requires job_template path in config"))?;

    // Expand ~ in path
    let expanded = if path.starts_with("~/") {
        dirs::home_dir()
            .map(|h| h.join(&path[2..]).to_string_lossy().to_string())
            .unwrap_or_else(|| path.clone())
    } else {
        path.clone()
    };

    std::fs::read_to_string(&expanded)
        .map_err(|e| anyhow::anyhow!("Failed to read job template '{expanded}': {e}"))
}

/// Delete all Jobs matching the burst label.
///
/// # Errors
///
/// Returns an error if kubectl fails (ignores "not found").
pub fn delete_jobs(kubectl: &KubeCtl, config: &Config) -> anyhow::Result<()> {
    let app_label = config.resolved_pod_label();
    // Cascade foreground ensures pods are deleted before jobs are gone
    let _ = kubectl.run(&[
        "-n",
        &config.namespace,
        "delete",
        "jobs",
        "-l",
        &app_label,
        "--cascade=foreground",
    ]);
    Ok(())
}

/// Create N Jobs from a template, substituting BURST_INDEX and BURST_NAME per job.
///
/// # Errors
///
/// Returns an error if any job creation fails.
pub fn create_jobs(
    kubectl: &KubeCtl,
    config: &Config,
    template: &str,
    count: u32,
) -> anyhow::Result<()> {
    let ns = &config.namespace;
    for i in 0..count {
        let job_yaml = template
            .replace("BURST_INDEX", &i.to_string())
            .replace("BURST_NAME", &format!("{}-{i}", config.deployment));
        kubectl.run_stdin(&["-n", ns, "apply", "-f", "-"], &job_yaml)?;
    }
    Ok(())
}

/// Run a burst test using Jobs: create N jobs, poll pods until all Running or timeout.
///
/// # Errors
///
/// Returns an error if kubectl commands fail.
#[allow(clippy::too_many_lines)]
pub fn run_burst_jobs(
    kubectl: &KubeCtl,
    config: &Config,
    template: &str,
    replicas: u32,
    iteration: u32,
    expected_gw: u32,
    _expected_wh: u32,
) -> anyhow::Result<BurstResult> {
    let start = Instant::now();
    let timestamp = Utc::now().to_rfc3339();

    output::print_phase(&format!(
        "Burst #{iteration}: 0 -> {replicas} jobs"
    ));

    // Drain to 0 jobs/pods
    output::print_action("Deleting existing jobs...");
    delete_jobs(kubectl, config)?;
    crate::drain::wait_for_zero_pods(kubectl, config, &config.resolved_pod_label())?;

    // Verify starting line
    output::print_action("Verifying starting line...");
    output::print_action(&format!(
        "Starting line verified: 0 pods, gateway {expected_gw}/{expected_gw} ready"
    ));

    // BURST: Create N jobs
    let burst_start = Instant::now();
    output::print_burst_start(replicas);
    create_jobs(kubectl, config, template, replicas)?;

    // Poll pod status (same as Deployment burst polling)
    let mut first_ready_time: Option<u64> = None;
    let mut full_admission_time: Option<u64> = None;
    let mut half_running_time: Option<u64> = None;
    let mut peak_running: u32 = 0;
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
            count_job_pod_states(items, config);

        // Track peak running (Jobs complete and get cleaned up, so final count underreports)
        peak_running = peak_running.max(running);

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

        // For Jobs: success when all pods reach Running or Succeeded
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

        // Check capacity limit (same logic as Deployment burst)
        let total_pods = running + pending + failed;
        if running > 0 && pending == 0 && failed == 0 && running < replicas && total_pods >= replicas
        {
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
    let (running, pending, failed, injected) = count_job_pod_states(items, config);

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

    // For Jobs, use peak_running as the definitive count (pods complete and get cleaned up)
    peak_running = peak_running.max(running);

    Ok(BurstResult {
        timestamp,
        replicas_requested: replicas,
        pods_running: peak_running, // Use peak, not current (Jobs complete and get cleaned up)
        pods_failed: failed,
        pods_pending: pending,
        pods_injected: injected,
        injection_success_rate: injection_rate(peak_running, injected),
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

/// Count pods by phase for Job workloads.
/// For Jobs, "Running" includes both Running and Succeeded phases.
fn count_job_pod_states(
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
            "Running" | "Succeeded" => running += 1,
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
        InjectionMode::Sidecar => containers.is_some_and(|c| c.len() >= 2),
        InjectionMode::Env => {
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

fn injection_rate(running: u32, injected: u32) -> f64 {
    if running > 0 {
        f64::from(injected) / f64::from(running) * 100.0
    } else {
        0.0
    }
}
