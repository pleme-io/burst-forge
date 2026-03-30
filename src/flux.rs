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
        "Waiting for {} kustomizations (timeout: {timeout_secs}s)...",
        kustomizations.len()
    );

    for ks_name in kustomizations {
        let start = Instant::now();
        println!("  Waiting for {ks_name}...");

        loop {
            if start.elapsed() > timeout {
                anyhow::bail!("Timeout waiting for kustomization {ks_name} to become Ready");
            }

            match check_kustomization_ready(kubectl, ks_name) {
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
fn check_kustomization_ready(kubectl: &KubeCtl, name: &str) -> anyhow::Result<bool> {
    let json = kubectl.get_json(&[
        "-n",
        "flux-system",
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
