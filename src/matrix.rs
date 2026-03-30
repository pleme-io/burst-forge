//! Scaling matrix — run multiple scenarios, collect results.

use chrono::Utc;

use crate::burst;
use crate::config::Config;
use crate::kubectl::KubeCtl;
use crate::types::{MatrixReport, ScenarioResult};
use crate::verify;

/// Run the scaling matrix: iterate scenarios, patch replicas, verify, burst.
///
/// # Errors
///
/// Returns an error if resetting replicas after all scenarios fails.
pub fn run_matrix(
    kubectl: &KubeCtl,
    config: &Config,
    scenario_filter: Option<&str>,
    skip_scaling: bool,
) -> anyhow::Result<MatrixReport> {
    let timestamp = Utc::now().to_rfc3339();

    if config.scenarios.is_empty() {
        anyhow::bail!("No scenarios configured. Add scenarios to your burst-forge.yaml config.");
    }

    let scenarios: Vec<_> = config
        .scenarios
        .iter()
        .filter(|s| {
            scenario_filter
                .is_none_or(|f| s.name == f)
        })
        .collect();

    if scenarios.is_empty() {
        anyhow::bail!(
            "No scenarios matched filter {scenario_filter:?}"
        );
    }

    println!(
        "\n=== Scaling Matrix: {} scenarios ===\n",
        scenarios.len()
    );

    let mut results = Vec::new();

    for scenario in &scenarios {
        println!(
            "\n--- Scenario: {} (replicas={}, gw={}, wh={}) ---",
            scenario.name,
            scenario.replicas,
            scenario.gateway_replicas,
            scenario.webhook_replicas,
        );

        let result = run_single_scenario(kubectl, config, scenario, skip_scaling);
        results.push(result);

        // Cool down between scenarios
        println!("\n  Cooling down 10s...");
        std::thread::sleep(std::time::Duration::from_secs(10));
    }

    // Reset replicas to defaults after all scenarios
    if !skip_scaling {
        println!("\n  Resetting HelmRelease replicas...");
        let _ = kubectl.patch_helmrelease_replicas(
            &config.akeyless_namespace,
            &config.gateway_release,
            1,
        );
        let _ = kubectl.patch_helmrelease_replicas(
            &config.akeyless_namespace,
            &config.webhook_release,
            1,
        );
    }

    Ok(MatrixReport {
        timestamp,
        scenarios: results,
    })
}

/// Run a single scenario and capture the result.
fn run_single_scenario(
    kubectl: &KubeCtl,
    config: &Config,
    scenario: &crate::config::Scenario,
    skip_scaling: bool,
) -> ScenarioResult {
    // Patch HelmRelease replicas (unless skipping)
    if !skip_scaling {
        println!("  Patching gateway replicas to {}...", scenario.gateway_replicas);
        if let Err(e) = kubectl.patch_helmrelease_replicas(
            &config.akeyless_namespace,
            &config.gateway_release,
            scenario.gateway_replicas,
        ) {
            return ScenarioResult {
                name: scenario.name.clone(),
                replicas: scenario.replicas,
                gateway_replicas: scenario.gateway_replicas,
                webhook_replicas: scenario.webhook_replicas,
                verify: None,
                burst: None,
                error: Some(format!("Failed to patch gateway: {e}")),
            };
        }

        println!(
            "  Patching webhook replicas to {}...",
            scenario.webhook_replicas
        );
        if let Err(e) = kubectl.patch_helmrelease_replicas(
            &config.akeyless_namespace,
            &config.webhook_release,
            scenario.webhook_replicas,
        ) {
            return ScenarioResult {
                name: scenario.name.clone(),
                replicas: scenario.replicas,
                gateway_replicas: scenario.gateway_replicas,
                webhook_replicas: scenario.webhook_replicas,
                verify: None,
                burst: None,
                error: Some(format!("Failed to patch webhook: {e}")),
            };
        }

        // Wait for rollout
        println!("  Waiting for rollout (30s)...");
        std::thread::sleep(std::time::Duration::from_secs(30));
    }

    // Verify infrastructure
    let verify_result = match verify::verify_infra(kubectl, config) {
        Ok(v) => Some(v),
        Err(e) => {
            return ScenarioResult {
                name: scenario.name.clone(),
                replicas: scenario.replicas,
                gateway_replicas: scenario.gateway_replicas,
                webhook_replicas: scenario.webhook_replicas,
                verify: None,
                burst: None,
                error: Some(format!("Verification failed: {e}")),
            };
        }
    };

    // Run burst
    let burst_result = match burst::run_burst(kubectl, config, scenario.replicas, 1) {
        Ok(b) => Some(b),
        Err(e) => {
            return ScenarioResult {
                name: scenario.name.clone(),
                replicas: scenario.replicas,
                gateway_replicas: scenario.gateway_replicas,
                webhook_replicas: scenario.webhook_replicas,
                verify: verify_result,
                burst: None,
                error: Some(format!("Burst failed: {e}")),
            };
        }
    };

    ScenarioResult {
        name: scenario.name.clone(),
        replicas: scenario.replicas,
        gateway_replicas: scenario.gateway_replicas,
        webhook_replicas: scenario.webhook_replicas,
        verify: verify_result,
        burst: burst_result,
        error: None,
    }
}
