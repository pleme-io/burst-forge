//! `FluxCD` kustomization readiness polling.

use std::time::{Duration, Instant};

use crate::kubectl::KubeCtl;
use crate::output;

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
        output::print_status("No kustomizations configured, skipping wait.");
        return Ok(());
    }

    let timeout = Duration::from_secs(timeout_secs);
    let poll = Duration::from_secs(poll_interval_secs);

    output::print_flux_header(kustomizations.len(), flux_namespace, timeout_secs);

    for ks_name in kustomizations {
        let start = Instant::now();
        output::print_flux_waiting(ks_name);

        loop {
            if start.elapsed() > timeout {
                anyhow::bail!(
                    "Timeout waiting for kustomization {flux_namespace}/{ks_name} to become Ready after {timeout_secs}s"
                );
            }

            match check_kustomization_ready(kubectl, flux_namespace, ks_name) {
                Ok(true) => {
                    let elapsed = start.elapsed().as_secs();
                    output::print_flux_ready(ks_name, elapsed);
                    break;
                }
                Ok(false) => {
                    std::thread::sleep(poll);
                }
                Err(e) => {
                    output::print_warning(&format!("{ks_name}: error checking status: {e}"));
                    // Back off on errors to avoid hammering a struggling API server
                    std::thread::sleep(poll * 2);
                }
            }
        }
    }

    output::print_flux_complete();
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
