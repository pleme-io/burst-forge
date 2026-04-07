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
    scenario_name: &str,
    emitter: &crate::events::EventEmitter,
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
    let mut peak_running: u32 = 0;
    let mut total_secrets: u32 = 0;
    let mut poll_count: u32 = 0;
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

        peak_running = peak_running.max(running);
        total_secrets = items.iter().map(|p| injection_secret_count(p, config)).sum();

        #[allow(clippy::cast_possible_truncation)]
        let elapsed_ms = burst_start.elapsed().as_millis() as u64;

        if running > 0 && first_ready_time.is_none() {
            first_ready_time = Some(elapsed_ms);
            emitter.milestone(scenario_name, "FIRST_READY", elapsed_ms, 1);
        }

        if running >= half_target && half_running_time.is_none() {
            half_running_time = Some(elapsed_ms);
            emitter.milestone(scenario_name, "50PCT_RUNNING", elapsed_ms, running);
        }

        if injected >= replicas && full_admission_time.is_none() {
            full_admission_time = Some(elapsed_ms);
            emitter.milestone(scenario_name, "FULL_ADMISSION", elapsed_ms, injected);
        }

        // Emit poll tick every 5th iteration
        if poll_count % 5 == 0 {
            emitter.poll_tick(scenario_name, running, pending, failed, injected, elapsed_ms, peak_running);
        }
        poll_count += 1;

        output::print_progress_ms(
            elapsed_ms,
            &format!(
                "Running: {running:>4} | Pending: {pending:>4} | Failed: {failed:>3} | Injected: {injected:>4}"
            ),
        );

        if running >= replicas {
            let rate = crate::types::injection_rate(running, injected);
            output::print_burst_complete(running, replicas, elapsed_ms, rate);
            // Emit detailed pod state for notable pods (restarts, failures)
            let pod_details: Vec<crate::types::PodDetail> = items.iter()
                .map(|p| crate::types::PodDetail::from_json(p, has_injection(p, config)))
                .collect();
            emitter.pod_state_detail(scenario_name, &pod_details);
            let admission_rate = crate::types::throughput_per_sec(injected, elapsed_ms);
            let gw_throughput = crate::types::throughput_per_sec(running, elapsed_ms);
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
                total_secrets_injected: total_secrets,
                peak_running,
                prediction: None,
            });
        }

        // Only declare capacity limit when ALL expected pods exist (no more being created)
        // AND none are pending. Otherwise the deployment controller is still creating pods.
        let total_pods = running + pending + failed;
        if running > 0 && pending == 0 && failed == 0 && running < replicas && total_pods >= replicas {
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
    let admission_rate = crate::types::throughput_per_sec(injected, final_elapsed_ms);
    let gw_throughput = crate::types::throughput_per_sec(running, final_elapsed_ms);

    Ok(BurstResult {
        timestamp,
        replicas_requested: replicas,
        pods_running: running,
        pods_failed: failed,
        pods_pending: pending,
        pods_injected: injected,
        injection_success_rate: crate::types::injection_rate(running, injected),
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
        total_secrets_injected: total_secrets,
        peak_running,
        prediction: None,
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
pub(crate) fn has_injection(pod: &serde_json::Value, config: &Config) -> bool {
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

/// Count individual injected secrets per pod (env vars matching prefix).
/// Returns the total count across all containers.
pub(crate) fn injection_secret_count(pod: &serde_json::Value, config: &Config) -> u32 {
    let Some(containers) = pod["spec"]["containers"].as_array() else {
        return 0;
    };
    let prefix = &config.injection_env_prefix;
    let mut count = 0u32;
    for c in containers {
        if let Some(envs) = c["env"].as_array() {
            for e in envs {
                if e["name"]
                    .as_str()
                    .is_some_and(|n| n.starts_with(prefix))
                {
                    count += 1;
                }
            }
        }
    }
    count
}

/// Apply per-scenario pod spec patches to the deployment before bursting.
///
/// Uses strategic merge patch (same pattern as maxSurge patching).
/// When a field is None, resets to baseline to ensure inter-scenario isolation.
pub fn apply_scenario_patches(
    kubectl: &crate::kubectl::KubeCtl,
    config: &crate::config::Config,
    scenario: &crate::config::Scenario,
) -> anyhow::Result<()> {
    let ns = &config.namespace;
    let dep = &config.deployment;

    // Init container latency: always set to ensure inter-scenario isolation
    let sleep = scenario.init_sleep_secs.unwrap_or(0);
    let init_cmd = if sleep > 0 {
        format!("sleep {sleep} && echo done")
    } else {
        "echo customer-init-complete".to_string()
    };
    crate::output::print_action(&format!("Setting init container: {init_cmd}"));
    let patch = serde_json::to_string(&serde_json::json!({
        "spec": {"template": {"spec": {"initContainers": [{
            "name": &config.init_container_name,
            "command": ["sh", "-c", &init_cmd]
        }]}}}
    }))?;
    kubectl.run(&["-n", ns, "patch", "deployment", dep, "--type=strategic", "-p", &patch])?;

    // Memory request/limit: always set to ensure inter-scenario isolation
    let container_name = &config.workload_container_name;
    if let Some(mem) = &scenario.pod_memory_request {
        let limit = scenario.pod_memory_limit.as_deref().unwrap_or(mem);
        crate::output::print_action(&format!("Setting pod memory: request={mem}, limit={limit}"));
        let patch = serde_json::to_string(&serde_json::json!({
            "spec": {"template": {"spec": {"containers": [{
                "name": container_name,
                "resources": {"requests": {"memory": mem}, "limits": {"memory": limit}}
            }]}}}
        }))?;
        kubectl.run(&["-n", ns, "patch", "deployment", dep, "--type=strategic", "-p", &patch])?;
    } else {
        // Reset to baseline (16Mi/64Mi from deployment template)
        let patch = serde_json::to_string(&serde_json::json!({
            "spec": {"template": {"spec": {"containers": [{
                "name": container_name,
                "resources": {"requests": {"memory": "16Mi"}, "limits": {"memory": "64Mi"}}
            }]}}}
        }))?;
        kubectl.run(&["-n", ns, "patch", "deployment", dep, "--type=strategic", "-p", &patch])?;
    }

    // Secret count: patch the akeyless/secret-path annotation with N comma-separated paths
    if let Some(secret_count) = scenario.expected_secrets {
        let prefix = &config.secret_path_prefix;
        let paths: Vec<String> = (1..=secret_count)
            .map(|i| {
                if i == 1 {
                    prefix.clone()
                } else {
                    format!("{prefix}{i}")
                }
            })
            .collect();
        let secret_path = paths.join(",");
        crate::output::print_action(&format!("Setting {secret_count} secrets: {secret_path}"));
        let patch = serde_json::to_string(&serde_json::json!({
            "spec": {"template": {"metadata": {"annotations": {
                "akeyless/secret-path": &secret_path
            }}}}
        }))?;
        kubectl.run(&["-n", ns, "patch", "deployment", dep, "--type=strategic", "-p", &patch])?;
    }

    Ok(())
}

/// Apply per-scenario infrastructure resource patches (webhook + gateway CPU).
///
/// Patches deployment container resources directly via kubectl.
/// HelmRelease is suspended at this point, so patches persist until resume.
pub fn apply_infrastructure_patches(
    kubectl: &crate::kubectl::KubeCtl,
    config: &crate::config::Config,
    scenario: &crate::config::Scenario,
) -> anyhow::Result<()> {
    let inj_ns = &config.injection_namespace;

    // Webhook CPU override
    if scenario.webhook_cpu_request.is_some() || scenario.webhook_cpu_limit.is_some() {
        let wh_dep = &config.webhook_deployment;
        let wh_name = &config.webhook_container_name;
        let req = scenario.webhook_cpu_request.as_deref().unwrap_or("50m");
        let limit = scenario.webhook_cpu_limit.as_deref().unwrap_or("200m");

        if limit == "0" {
            crate::output::print_action(&format!("Setting WH CPU: request={req}, limit=NONE"));
            let patch = serde_json::to_string(&serde_json::json!({
                "spec": {"template": {"spec": {"containers": [{
                    "name": wh_name,
                    "resources": {"requests": {"cpu": req}, "limits": {}}
                }]}}}
            }))?;
            kubectl.run(&["-n", inj_ns, "patch", "deployment", wh_dep, "--type=strategic", "-p", &patch])?;
        } else {
            crate::output::print_action(&format!("Setting WH CPU: request={req}, limit={limit}"));
            let patch = serde_json::to_string(&serde_json::json!({
                "spec": {"template": {"spec": {"containers": [{
                    "name": wh_name,
                    "resources": {"requests": {"cpu": req}, "limits": {"cpu": limit}}
                }]}}}
            }))?;
            kubectl.run(&["-n", inj_ns, "patch", "deployment", wh_dep, "--type=strategic", "-p", &patch])?;
        }
    }

    // Gateway CPU override
    if scenario.gateway_cpu_request.is_some() || scenario.gateway_cpu_limit.is_some() {
        let gw_dep = &config.gateway_deployment;
        let gw_name = &config.gateway_container_name;
        let req = scenario.gateway_cpu_request.as_deref().unwrap_or("100m");
        let limit = scenario.gateway_cpu_limit.as_deref().unwrap_or("500m");

        if limit == "0" {
            crate::output::print_action(&format!("Setting GW CPU: request={req}, limit=NONE"));
            let patch = serde_json::to_string(&serde_json::json!({
                "spec": {"template": {"spec": {"containers": [{
                    "name": gw_name,
                    "resources": {"requests": {"cpu": req}, "limits": {}}
                }]}}}
            }))?;
            kubectl.run(&["-n", inj_ns, "patch", "deployment", gw_dep, "--type=strategic", "-p", &patch])?;
        } else {
            crate::output::print_action(&format!("Setting GW CPU: request={req}, limit={limit}"));
            let patch = serde_json::to_string(&serde_json::json!({
                "spec": {"template": {"spec": {"containers": [{
                    "name": gw_name,
                    "resources": {"requests": {"cpu": req}, "limits": {"cpu": limit}}
                }]}}}
            }))?;
            kubectl.run(&["-n", inj_ns, "patch", "deployment", gw_dep, "--type=strategic", "-p", &patch])?;
        }
    }

    // Gateway memory override.
    //
    // When only `gateway_memory_limit` is specified, default the REQUEST to
    // the same value as the LIMIT. The previous default (`256Mi`) caused the
    // scheduler to pack 16 GW pods onto a single m5.large because it only
    // sees the request, then the kernel OOM-killed pods at runtime when their
    // actual usage exceeded the per-node RAM ceiling — Phase 1e gw16 spent
    // 339s in an OOM-restart loop before stabilizing. Setting request = limit
    // forces the scheduler to spread pods proportional to real memory usage
    // and avoids the entire flap.
    if scenario.gateway_memory_request.is_some() || scenario.gateway_memory_limit.is_some() {
        let gw_dep = &config.gateway_deployment;
        let gw_name = &config.gateway_container_name;
        let limit = scenario.gateway_memory_limit.as_deref().unwrap_or("1536Mi");
        let req = scenario.gateway_memory_request.as_deref().unwrap_or(limit);
        crate::output::print_action(&format!("Setting GW memory: request={req}, limit={limit}"));
        let patch = serde_json::to_string(&serde_json::json!({
            "spec": {"template": {"spec": {"containers": [{
                "name": gw_name,
                "resources": {"requests": {"memory": req}, "limits": {"memory": limit}}
            }]}}}
        }))?;
        kubectl.run(&["-n", inj_ns, "patch", "deployment", gw_dep, "--type=strategic", "-p", &patch])?;
    }

    // Webhook memory override — same request=limit default for the same reason.
    if scenario.webhook_memory_request.is_some() || scenario.webhook_memory_limit.is_some() {
        let wh_dep = &config.webhook_deployment;
        let wh_name = &config.webhook_container_name;
        let limit = scenario.webhook_memory_limit.as_deref().unwrap_or("256Mi");
        let req = scenario.webhook_memory_request.as_deref().unwrap_or(limit);
        crate::output::print_action(&format!("Setting WH memory: request={req}, limit={limit}"));
        let patch = serde_json::to_string(&serde_json::json!({
            "spec": {"template": {"spec": {"containers": [{
                "name": wh_name,
                "resources": {"requests": {"memory": req}, "limits": {"memory": limit}}
            }]}}}
        }))?;
        kubectl.run(&["-n", inj_ns, "patch", "deployment", wh_dep, "--type=strategic", "-p", &patch])?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_mode_config() -> Config {
        serde_json::from_str(r#"{
            "injection_mode": "env",
            "injection_env_prefix": "AKEYLESS_"
        }"#).unwrap()
    }

    fn sidecar_mode_config() -> Config {
        serde_json::from_str(r#"{
            "injection_mode": "sidecar"
        }"#).unwrap()
    }

    #[test]
    fn has_injection_env_mode_with_matching_env() {
        let config = env_mode_config();
        let pod = serde_json::json!({
            "spec": {
                "containers": [{
                    "name": "nginx",
                    "env": [
                        {"name": "AKEYLESS_SECRET_1", "value": "val1"},
                        {"name": "OTHER_VAR", "value": "val2"}
                    ]
                }]
            }
        });
        assert!(has_injection(&pod, &config));
    }

    #[test]
    fn has_injection_env_mode_no_matching_env() {
        let config = env_mode_config();
        let pod = serde_json::json!({
            "spec": {
                "containers": [{
                    "name": "nginx",
                    "env": [
                        {"name": "OTHER_VAR", "value": "val1"}
                    ]
                }]
            }
        });
        assert!(!has_injection(&pod, &config));
    }

    #[test]
    fn has_injection_env_mode_no_env_array() {
        let config = env_mode_config();
        let pod = serde_json::json!({
            "spec": {
                "containers": [{"name": "nginx"}]
            }
        });
        assert!(!has_injection(&pod, &config));
    }

    #[test]
    fn has_injection_env_mode_no_containers() {
        let config = env_mode_config();
        let pod = serde_json::json!({"spec": {}});
        assert!(!has_injection(&pod, &config));
    }

    #[test]
    fn has_injection_env_mode_empty_containers() {
        let config = env_mode_config();
        let pod = serde_json::json!({
            "spec": {"containers": []}
        });
        assert!(!has_injection(&pod, &config));
    }

    #[test]
    fn has_injection_env_mode_multiple_containers_second_has_env() {
        let config = env_mode_config();
        let pod = serde_json::json!({
            "spec": {
                "containers": [
                    {"name": "nginx", "env": [{"name": "OTHER", "value": "v"}]},
                    {"name": "sidecar", "env": [{"name": "AKEYLESS_PATH", "value": "/secret"}]}
                ]
            }
        });
        assert!(has_injection(&pod, &config));
    }

    #[test]
    fn has_injection_sidecar_mode_single_container() {
        let config = sidecar_mode_config();
        let pod = serde_json::json!({
            "spec": {"containers": [{"name": "nginx"}]}
        });
        assert!(!has_injection(&pod, &config));
    }

    #[test]
    fn has_injection_sidecar_mode_two_containers() {
        let config = sidecar_mode_config();
        let pod = serde_json::json!({
            "spec": {"containers": [{"name": "nginx"}, {"name": "sidecar"}]}
        });
        assert!(has_injection(&pod, &config));
    }

    #[test]
    fn has_injection_sidecar_mode_three_containers() {
        let config = sidecar_mode_config();
        let pod = serde_json::json!({
            "spec": {"containers": [{"name": "a"}, {"name": "b"}, {"name": "c"}]}
        });
        assert!(has_injection(&pod, &config));
    }

    #[test]
    fn has_injection_sidecar_mode_no_containers() {
        let config = sidecar_mode_config();
        let pod = serde_json::json!({"spec": {}});
        assert!(!has_injection(&pod, &config));
    }

    #[test]
    fn injection_secret_count_multiple_secrets() {
        let config = env_mode_config();
        let pod = serde_json::json!({
            "spec": {
                "containers": [{
                    "name": "nginx",
                    "env": [
                        {"name": "AKEYLESS_SECRET_1", "value": "v1"},
                        {"name": "AKEYLESS_SECRET_2", "value": "v2"},
                        {"name": "OTHER_VAR", "value": "v3"}
                    ]
                }]
            }
        });
        assert_eq!(injection_secret_count(&pod, &config), 2);
    }

    #[test]
    fn injection_secret_count_zero() {
        let config = env_mode_config();
        let pod = serde_json::json!({
            "spec": {
                "containers": [{
                    "name": "nginx",
                    "env": [{"name": "OTHER", "value": "v"}]
                }]
            }
        });
        assert_eq!(injection_secret_count(&pod, &config), 0);
    }

    #[test]
    fn injection_secret_count_no_containers() {
        let config = env_mode_config();
        let pod = serde_json::json!({"spec": {}});
        assert_eq!(injection_secret_count(&pod, &config), 0);
    }

    #[test]
    fn injection_secret_count_across_containers() {
        let config = env_mode_config();
        let pod = serde_json::json!({
            "spec": {
                "containers": [
                    {"name": "c1", "env": [{"name": "AKEYLESS_A", "value": "v"}]},
                    {"name": "c2", "env": [{"name": "AKEYLESS_B", "value": "v"}, {"name": "AKEYLESS_C", "value": "v"}]}
                ]
            }
        });
        assert_eq!(injection_secret_count(&pod, &config), 3);
    }

    #[test]
    fn injection_secret_count_no_env_on_container() {
        let config = env_mode_config();
        let pod = serde_json::json!({
            "spec": {"containers": [{"name": "nginx"}]}
        });
        assert_eq!(injection_secret_count(&pod, &config), 0);
    }

    #[test]
    fn count_pod_states_mixed_phases() {
        let config = env_mode_config();
        let items = vec![
            serde_json::json!({"status": {"phase": "Running"}, "spec": {"containers": [{"name": "n", "env": [{"name": "AKEYLESS_X", "value": "v"}]}]}}),
            serde_json::json!({"status": {"phase": "Running"}, "spec": {"containers": [{"name": "n"}]}}),
            serde_json::json!({"status": {"phase": "Pending"}, "spec": {"containers": [{"name": "n"}]}}),
            serde_json::json!({"status": {"phase": "Failed"}, "spec": {"containers": [{"name": "n"}]}}),
            serde_json::json!({"status": {"phase": "Succeeded"}, "spec": {"containers": [{"name": "n"}]}}),
        ];
        let (running, pending, failed, injected) = count_pod_states(&items, &config);
        assert_eq!(running, 2);
        assert_eq!(pending, 1);
        assert_eq!(failed, 1);
        assert_eq!(injected, 1);
    }

    #[test]
    fn count_pod_states_empty() {
        let config = env_mode_config();
        let items: Vec<serde_json::Value> = vec![];
        let (running, pending, failed, injected) = count_pod_states(&items, &config);
        assert_eq!(running, 0);
        assert_eq!(pending, 0);
        assert_eq!(failed, 0);
        assert_eq!(injected, 0);
    }

    #[test]
    fn count_pod_states_all_running_all_injected() {
        let config = env_mode_config();
        let items: Vec<serde_json::Value> = (0..5)
            .map(|_| serde_json::json!({
                "status": {"phase": "Running"},
                "spec": {"containers": [{"name": "n", "env": [{"name": "AKEYLESS_S", "value": "v"}]}]}
            }))
            .collect();
        let (running, pending, failed, injected) = count_pod_states(&items, &config);
        assert_eq!(running, 5);
        assert_eq!(pending, 0);
        assert_eq!(failed, 0);
        assert_eq!(injected, 5);
    }

    #[test]
    fn count_pod_states_unknown_phase_ignored() {
        let config = env_mode_config();
        let items = vec![
            serde_json::json!({"status": {"phase": "Unknown"}, "spec": {"containers": []}}),
            serde_json::json!({"status": {}, "spec": {"containers": []}}),
        ];
        let (running, pending, failed, injected) = count_pod_states(&items, &config);
        assert_eq!(running, 0);
        assert_eq!(pending, 0);
        assert_eq!(failed, 0);
        assert_eq!(injected, 0);
    }

    #[test]
    fn has_injection_env_prefix_exact_match() {
        let config: Config = serde_json::from_str(r#"{
            "injection_mode": "env",
            "injection_env_prefix": "MY_PREFIX_"
        }"#).unwrap();
        let pod = serde_json::json!({
            "spec": {"containers": [{"name": "c", "env": [{"name": "MY_PREFIX_SECRET", "value": "v"}]}]}
        });
        assert!(has_injection(&pod, &config));
    }

    #[test]
    fn has_injection_env_prefix_no_match_partial() {
        let config: Config = serde_json::from_str(r#"{
            "injection_mode": "env",
            "injection_env_prefix": "MY_PREFIX_"
        }"#).unwrap();
        let pod = serde_json::json!({
            "spec": {"containers": [{"name": "c", "env": [{"name": "MY_PREFIX", "value": "v"}]}]}
        });
        assert!(!has_injection(&pod, &config));
    }
}

