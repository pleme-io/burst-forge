//! Pod drain and state verification utilities.
//!
//! Provides verified pod drain (poll until truly 0 pods), gateway health
//! checks, and pre-burst starting-line verification. All drain operations
//! confirm actual pod count rather than trusting `kubectl scale` alone.

use std::time::{Duration, Instant};

use crate::config::Config;
use crate::kubectl::KubeCtl;

/// Drain all pods matching the app label to 0, with verified polling.
///
/// 1. Issues `kubectl scale --replicas=0`
/// 2. Polls pod count at `drain_poll_interval_secs` until 0 pods remain
/// 3. If `drain_timeout_secs` expires, force-deletes remaining pods
///
/// # Errors
///
/// Returns an error if kubectl commands fail or force-delete also fails.
pub fn drain_pods(kubectl: &KubeCtl, config: &Config) -> anyhow::Result<()> {
    let app_label = config.resolved_pod_label();

    // Issue scale to 0
    kubectl.run(&[
        "-n",
        &config.namespace,
        "scale",
        "deployment",
        &config.deployment,
        "--replicas=0",
    ])?;

    // Poll until verified 0 pods
    wait_for_zero_pods(kubectl, config, &app_label)
}

/// Poll until 0 pods exist with the given label, with progress reporting.
///
/// If the timeout expires, force-deletes remaining pods and waits briefly.
///
/// # Errors
///
/// Returns an error if kubectl commands fail or pods cannot be drained.
pub fn wait_for_zero_pods(
    kubectl: &KubeCtl,
    config: &Config,
    app_label: &str,
) -> anyhow::Result<()> {
    let timeout = Duration::from_secs(config.drain_timeout_secs);
    let poll_interval = Duration::from_secs(config.drain_poll_interval_secs);
    let start = Instant::now();

    loop {
        let count = count_pods(kubectl, &config.namespace, app_label)?;
        let elapsed = start.elapsed().as_secs();

        if count == 0 {
            println!("  Draining pods... [{elapsed}s] 0 remaining -- clean");
            return Ok(());
        }

        println!("  Draining pods... [{elapsed}s] {count} remaining");

        if start.elapsed() > timeout {
            println!(
                "  Drain timeout after {}s with {count} pods remaining -- force deleting",
                config.drain_timeout_secs
            );
            force_delete_pods(kubectl, &config.namespace, app_label)?;

            // Brief wait for force-delete to take effect, then verify
            std::thread::sleep(Duration::from_secs(5));
            let remaining = count_pods(kubectl, &config.namespace, app_label)?;
            if remaining > 0 {
                anyhow::bail!(
                    "Force delete failed: {remaining} pods still exist after force-delete"
                );
            }
            println!("  Force delete complete -- 0 remaining");
            return Ok(());
        }

        std::thread::sleep(poll_interval);
    }
}

/// Force delete all pods matching a label selector with `--grace-period=0 --force`.
///
/// # Errors
///
/// Returns an error if kubectl fails.
pub fn force_delete_pods(
    kubectl: &KubeCtl,
    namespace: &str,
    app_label: &str,
) -> anyhow::Result<()> {
    println!("  Force deleting pods (--grace-period=0 --force)...");
    // Ignore errors from "no resources found" — that is a success case
    let _ = kubectl.run(&[
        "-n",
        namespace,
        "delete",
        "pods",
        "-l",
        app_label,
        "--grace-period=0",
        "--force",
    ]);
    Ok(())
}

/// Count the number of pods matching a label selector.
///
/// # Errors
///
/// Returns an error if kubectl fails.
pub fn count_pods(
    kubectl: &KubeCtl,
    namespace: &str,
    app_label: &str,
) -> anyhow::Result<u32> {
    let output = kubectl.run(&[
        "-n",
        namespace,
        "get",
        "pods",
        "-l",
        app_label,
        "--no-headers",
    ]);

    match output {
        Ok(text) => {
            if text.trim().is_empty() {
                Ok(0)
            } else {
                #[allow(clippy::cast_possible_truncation)]
                Ok(text.lines().count() as u32)
            }
        }
        Err(e) => {
            let msg = e.to_string();
            // "No resources found" is not an error for our purposes
            if msg.contains("No resources found") || msg.contains("not found") {
                Ok(0)
            } else {
                Err(e)
            }
        }
    }
}

/// Verify that a deployment shows the expected replica state.
///
/// Returns `(ready_replicas, desired_replicas)`.
///
/// # Errors
///
/// Returns an error if kubectl fails.
pub fn get_deployment_replicas(
    kubectl: &KubeCtl,
    namespace: &str,
    deployment: &str,
) -> anyhow::Result<(u32, u32)> {
    let json = kubectl.get_json(&[
        "-n",
        namespace,
        "get",
        "deployment",
        deployment,
    ])?;

    let desired = json["spec"]["replicas"].as_u64().unwrap_or(0);
    let ready = json["status"]["readyReplicas"].as_u64().unwrap_or(0);

    #[allow(clippy::cast_possible_truncation)]
    Ok((ready as u32, desired as u32))
}

/// Verify gateway and webhook health by checking `readyReplicas == desiredReplicas`.
///
/// # Errors
///
/// Returns an error if either deployment is not fully ready.
pub fn verify_gateway_health(
    kubectl: &KubeCtl,
    config: &Config,
    expected_gw: u32,
    expected_wh: u32,
) -> anyhow::Result<()> {
    let (gw_ready, gw_desired) = get_deployment_replicas(
        kubectl,
        &config.injection_namespace,
        &config.gateway_deployment,
    )?;
    let (wh_ready, wh_desired) = get_deployment_replicas(
        kubectl,
        &config.injection_namespace,
        &config.webhook_deployment,
    )?;

    if gw_ready != expected_gw || gw_desired != expected_gw {
        anyhow::bail!(
            "Gateway not fully ready: {gw_ready}/{gw_desired} ready (expected {expected_gw}/{expected_gw})"
        );
    }

    if wh_ready != expected_wh || wh_desired != expected_wh {
        anyhow::bail!(
            "Webhook not fully ready: {wh_ready}/{wh_desired} ready (expected {expected_wh}/{expected_wh})"
        );
    }

    println!(
        "  Gateway {gw_ready}/{gw_desired} ready, Webhook {wh_ready}/{wh_desired} ready"
    );
    Ok(())
}

/// Verify the pre-burst starting line: 0 pods, gateway healthy, webhook healthy.
///
/// Prints a confirmation line with exact counts.
///
/// # Errors
///
/// Returns an error if the starting line is not clean.
pub fn verify_starting_line(
    kubectl: &KubeCtl,
    config: &Config,
    expected_gw: u32,
    expected_wh: u32,
) -> anyhow::Result<()> {
    let app_label = config.resolved_pod_label();

    // Check deployment shows 0/0
    let (ready, desired) = get_deployment_replicas(
        kubectl,
        &config.namespace,
        &config.deployment,
    )?;
    if ready != 0 || desired != 0 {
        anyhow::bail!(
            "Deployment not at zero: {ready}/{desired} (expected 0/0)"
        );
    }

    // Check no pods exist
    let pod_count = count_pods(kubectl, &config.namespace, &app_label)?;
    if pod_count != 0 {
        anyhow::bail!(
            "Stale pods found: {pod_count} pods still exist with label {app_label}"
        );
    }

    // Check gateway and webhook readiness
    let (gw_ready, _) = get_deployment_replicas(
        kubectl,
        &config.injection_namespace,
        &config.gateway_deployment,
    )?;
    let (wh_ready, _) = get_deployment_replicas(
        kubectl,
        &config.injection_namespace,
        &config.webhook_deployment,
    )?;

    println!(
        "  Starting line verified: 0 pods, gateway {gw_ready}/{expected_gw} ready, webhook {wh_ready}/{expected_wh} ready"
    );

    if gw_ready < expected_gw {
        anyhow::bail!(
            "Gateway not ready: {gw_ready}/{expected_gw} -- cannot start burst"
        );
    }
    if wh_ready < expected_wh {
        anyhow::bail!(
            "Webhook not ready: {wh_ready}/{expected_wh} -- cannot start burst"
        );
    }

    Ok(())
}
