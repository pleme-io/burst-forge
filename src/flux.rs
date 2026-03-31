//! `FluxCD` kustomization readiness polling.

use std::time::{Duration, Instant};

use crate::kubectl::KubeCtl;

/// Wait for all listed kustomizations to become Ready, in order.
///
/// Polls each kustomization's Ready condition at the given interval.
/// Fails fast if any kustomization fails to become Ready before the timeout.
///
/// # Errors
///
/// Returns an error if any kustomization does not reach Ready within the timeout.
pub fn wait_for_kustomizations(
    kubectl: &KubeCtl,
    flux_namespace: &str,
    kustomizations: &[String],
    timeout_secs: u64,
    poll_interval_secs: u64,
) -> anyhow::Result<()> {
    if kustomizations.is_empty() {
        println!("No kustomizations configured, skipping wait.");
        return Ok(());
    }

    let timeout = Duration::from_secs(timeout_secs);
    let poll = Duration::from_secs(poll_interval_secs);

    println!(
        "Waiting for {} kustomizations in namespace {flux_namespace} (timeout: {timeout_secs}s)...",
        kustomizations.len()
    );

    for ks_name in kustomizations {
        let start = Instant::now();
        println!("  Waiting for {ks_name}...");

        loop {
            if start.elapsed() > timeout {
                anyhow::bail!(
                    "Timeout waiting for kustomization {flux_namespace}/{ks_name} to become Ready after {timeout_secs}s"
                );
            }

            match check_kustomization_ready(kubectl, flux_namespace, ks_name) {
                Ok(true) => {
                    let elapsed = start.elapsed().as_secs();
                    println!("  {ks_name}: Ready ({elapsed}s)");
                    break;
                }
                Ok(false) => {}
                Err(e) => {
                    println!("  {ks_name}: error checking status: {e}");
                }
            }

            std::thread::sleep(poll);
        }
    }

    println!("All kustomizations ready.");
    Ok(())
}

/// Check if a single kustomization has Ready=True.
fn check_kustomization_ready(kubectl: &KubeCtl, namespace: &str, name: &str) -> anyhow::Result<bool> {
    let json = kubectl.get_json(&[
        "-n",
        namespace,
        "get",
        "kustomization",
        name,
    ])?;

    let conditions = json["status"]["conditions"].as_array();
    let Some(conditions) = conditions else {
        return Ok(false);
    };

    let ready = conditions.iter().any(|c| {
        c["type"].as_str() == Some("Ready") && c["status"].as_str() == Some("True")
    });

    Ok(ready)
}
