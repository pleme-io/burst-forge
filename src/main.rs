//! burst-forge -- Kubernetes burst test orchestrator.
//!
//! Coordinated pod scaling with configurable injection verification.
//! Designed for scale testing: 0 -> N pods with real secret injection,
//! measuring timing and scraping injection success/failure.

mod burst;
mod config;
mod drain;
mod flux;
mod gates;
mod kubectl;
mod matrix;
mod nodes;
mod output;
mod phases;
mod report;
mod types;
mod verify;

use clap::{Parser, Subcommand};
use kubectl::KubeCtl;

#[derive(Parser)]
#[command(
    name = "burst-forge",
    about = "Kubernetes burst test orchestrator",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Kubeconfig path
    #[arg(long, global = true, env = "KUBECONFIG")]
    kubeconfig: Option<String>,

    /// Path to burst-forge YAML config
    #[arg(long, global = true)]
    config: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Verify infrastructure is ready for burst testing
    Verify,

    /// Wait for `FluxCD` kustomizations to become Ready
    Wait,

    /// Run a burst test: scale 0 -> N, measure, report
    Burst {
        /// Number of replicas to burst to (overrides config)
        #[arg(long)]
        replicas: Option<u32>,

        /// Number of burst iterations
        #[arg(long, default_value = "1")]
        iterations: u32,
    },

    /// Run the scaling matrix across all configured scenarios
    Matrix {
        /// Run only a specific scenario by name
        #[arg(long)]
        scenario: Option<String>,

        /// Skip `HelmRelease` replica patching
        #[arg(long)]
        skip_scaling: bool,
    },

    /// Reset deployment to 0 replicas
    Reset,

    /// Reset entire environment to starting conditions
    /// Scales deployment to 0, drains all pods, resets gateway/webhook to 1,
    /// resumes `HelmReleases`, scales burst nodes to 0
    ResetAll,

    /// Manage EKS node group for burst testing
    Nodes {
        #[command(subcommand)]
        action: NodesAction,
    },

    /// Publish a JSON results file to Confluence
    Report {
        /// Path to the matrix results JSON file
        #[arg(long)]
        json: String,
    },

    /// Run a named flow from configs/ directory
    ///
    /// Discovers config files at `configs/{name}.yaml` relative to CWD.
    /// Equivalent to `matrix --config configs/{name}.yaml`.
    Flow {
        /// Flow name (maps to configs/{name}.yaml)
        name: String,

        /// Run only a specific scenario by name
        #[arg(long)]
        scenario: Option<String>,

        /// Skip `HelmRelease` replica patching
        #[arg(long)]
        skip_scaling: bool,
    },
}

#[derive(Subcommand)]
enum NodesAction {
    /// Scale node group up to a specific count
    Up {
        /// Number of nodes to scale to
        #[arg(long)]
        count: u32,
    },

    /// Scale node group down to 0
    Down,

    /// Show current node group status
    Status,
}

#[allow(clippy::too_many_lines)]
fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    // Resolve config path: --config flag, or flow name → configs/{name}.yaml
    let config_path = match &cli.command {
        Commands::Flow { name, .. } => {
            let path = format!("configs/{name}.yaml");
            if !std::path::Path::new(&path).exists() {
                anyhow::bail!(
                    "Flow config not found: {path}\n\
                     Available flows: {}",
                    list_flows().unwrap_or_else(|_| "none".to_string())
                );
            }
            Some(path)
        }
        _ => cli.config.clone(),
    };

    let cfg = config::discover(config_path.as_deref())?;

    // Config-level kubeconfig overrides CLI/env. Expand ~ to home dir.
    let kubeconfig = cfg.kubeconfig.clone()
        .or_else(|| cli.kubeconfig.clone())
        .map(|p| {
            if p.starts_with("~/") {
                dirs::home_dir()
                    .map(|h| h.join(&p[2..]).to_string_lossy().to_string())
                    .unwrap_or(p)
            } else {
                p
            }
        });
    let kubectl = KubeCtl::new(kubeconfig.clone());

    // Signal handler: graceful cleanup on Ctrl+C
    // Force-deletes burst pods, waits briefly for drain, then scales nodes to 0
    let cleanup_cfg = cfg.clone();
    let cleanup_kubeconfig = kubeconfig.clone();
    ctrlc::set_handler(move || {
        output::print_sigint_header();
        let kctl = KubeCtl::new(cleanup_kubeconfig.clone());
        let app_label = cleanup_cfg.resolved_pod_label();

        // Scale deployment to 0
        let _ = kctl.run(&[
            "-n", &cleanup_cfg.namespace,
            "scale", "deployment", &cleanup_cfg.deployment, "--replicas=0",
        ]);
        output::eprint_status("Deployment scaled to 0");

        // Force delete all burst pods (--grace-period=0)
        output::eprint_status("Force deleting burst pods...");
        let _ = kctl.run(&[
            "-n", &cleanup_cfg.namespace,
            "delete", "pods", "-l", &app_label,
            "--grace-period=0", "--force",
        ]);

        // Brief wait for drain to take effect
        std::thread::sleep(std::time::Duration::from_secs(5));

        // Verify pod count
        let remaining = drain::count_pods(&kctl, &cleanup_cfg.namespace, &app_label)
            .unwrap_or(u32::MAX);
        if remaining == 0 {
            output::eprint_status("All burst pods terminated");
        } else {
            output::eprint_warning(&format!("{remaining} pods may still be terminating"));
        }

        // Resume suspended kustomizations
        for ks in &cleanup_cfg.suspend_kustomizations {
            let _ = kctl.run(&[
                "-n", &cleanup_cfg.suspend_kustomizations_namespace,
                "patch", "kustomization", ks,
                "--type=merge", "-p", r#"{"spec":{"suspend":false}}"#,
            ]);
        }
        if !cleanup_cfg.suspend_kustomizations.is_empty() {
            output::eprint_status("Kustomizations resumed");
        }

        // Scale node group to 0
        if let Some(ng) = &cleanup_cfg.node_group {
            let _ = nodes::scale_node_group(ng, 0);
            output::eprint_status("Node group scaling to 0");
        }

        output::eprint_complete();
        std::process::exit(130);
    }).expect("failed to set Ctrl+C handler");

    match cli.command {
        Commands::Verify => {
            output::print_banner("Verify");
            let result = verify::verify_infra(&kubectl, &cfg)?;
            println!("\n{}", serde_json::to_string_pretty(&result)?);
        }

        Commands::Wait => {
            output::print_banner("FluxCD Wait");
            let kustomizations = cfg.resolved_flux_kustomizations();
            let flux_ns = cfg.flux_namespace();
            flux::wait_for_kustomizations(
                &kubectl,
                &flux_ns,
                kustomizations,
                cfg.timeout_secs,
                cfg.poll_interval_secs,
            )?;
        }

        Commands::Burst {
            replicas,
            iterations,
        } => {
            let target_replicas = replicas.unwrap_or(50);
            output::print_banner(&format!("Burst Test ({target_replicas} replicas x {iterations} iterations)"));

            verify::verify_infra(&kubectl, &cfg)?;

            // Detect current gateway/webhook replica counts for starting-line verification
            let (gw_ready, _) = drain::get_deployment_replicas(
                &kubectl,
                &cfg.injection_namespace,
                &cfg.gateway_deployment,
            ).unwrap_or((1, 1));
            let (wh_ready, _) = drain::get_deployment_replicas(
                &kubectl,
                &cfg.injection_namespace,
                &cfg.webhook_deployment,
            ).unwrap_or((1, 1));

            let mut results = Vec::new();
            for i in 1..=iterations {
                let result = burst::run_burst(
                    &kubectl, &cfg, target_replicas, i, gw_ready, wh_ready,
                )?;

                output::print_iteration_results(
                    i,
                    result.pods_running,
                    result.replicas_requested,
                    result.injection_success_rate,
                    result.time_to_first_ready_ms,
                    result.time_to_all_ready_ms,
                    result.duration_ms,
                );
                results.push(result);

                if i < iterations {
                    // Drain between iterations
                    output::print_action("Draining pods before next iteration...");
                    drain::drain_pods(&kubectl, &cfg)?;
                    output::print_cooldown(cfg.cooldown_secs);
                    std::thread::sleep(std::time::Duration::from_secs(cfg.cooldown_secs));
                }
            }

            output::print_phase("Burst Test Summary");
            println!("{}", serde_json::to_string_pretty(&results)?);
        }

        Commands::Matrix {
            scenario,
            skip_scaling,
        } => {
            output::print_banner("Scale Matrix");
            let matrix_report =
                matrix::run_matrix(&kubectl, &cfg, scenario.as_deref(), skip_scaling)?;

            output::print_phase("Matrix Report");
            println!("{}", serde_json::to_string_pretty(&matrix_report)?);

            // Publish to Confluence if configured — failure is fatal
            if let Some(conf) = &cfg.confluence {
                publish_report(conf, &matrix_report, &cfg)?;
            }
        }

        Commands::Reset => {
            output::print_banner("Reset");
            // Idempotent reset: ignore errors from missing deployments
            match kubectl.run(&[
                "-n",
                &cfg.namespace,
                "scale",
                "deployment",
                &cfg.deployment,
                "--replicas=0",
            ]) {
                Ok(_) => output::print_status(&format!(
                    "Reset {} to 0 replicas {}",
                    cfg.deployment,
                    output::bold_green("OK")
                )),
                Err(e) => {
                    output::print_warning(&format!(
                        "Could not reset deployment {}/{}: {e}",
                        cfg.namespace, cfg.deployment,
                    ));
                    output::print_status("This is expected if the deployment does not exist yet.");
                }
            }
        }

        Commands::ResetAll => {
            output::print_banner("Reset All");
            output::print_reset_header();

            // 1. Scale deployment to 0
            output::print_action(&format!("Scaling {} to 0...", cfg.deployment));
            let _ = kubectl.run(&["-n", &cfg.namespace, "scale", "deployment", &cfg.deployment, "--replicas=0"]);

            // 2. Drain all pods
            output::print_action("Draining pods...");
            let app_label = cfg.resolved_pod_label();
            let _ = drain::wait_for_zero_pods(&kubectl, &cfg, &app_label);

            // 3. Reset gateway/webhook to 1 replica
            output::print_action("Resetting gateway to 1 replica...");
            let _ = kubectl.run(&["-n", &cfg.injection_namespace, "scale", "deployment", &cfg.gateway_deployment, "--replicas=1"]);
            output::print_action("Resetting webhook to 1 replica...");
            let _ = kubectl.run(&["-n", &cfg.injection_namespace, "scale", "deployment", &cfg.webhook_deployment, "--replicas=1"]);

            // 4. Resume HelmReleases + kustomizations
            output::print_action("Resuming HelmReleases...");
            let _ = kubectl.run(&["-n", &cfg.injection_namespace, "patch", "helmrelease", &cfg.gateway_release, "--type=merge", "-p", r#"{"spec":{"suspend":false}}"#]);
            let _ = kubectl.run(&["-n", &cfg.injection_namespace, "patch", "helmrelease", &cfg.webhook_release, "--type=merge", "-p", r#"{"spec":{"suspend":false}}"#]);
            for ks in &cfg.suspend_kustomizations {
                output::print_action(&format!("Resuming kustomization {ks}..."));
                let _ = kubectl.run(&["-n", &cfg.suspend_kustomizations_namespace, "patch", "kustomization", ks, "--type=merge", "-p", r#"{"spec":{"suspend":false}}"#]);
            }

            // 5. Scale burst nodes to 0
            if let Some(ng) = &cfg.node_group {
                output::print_action("Scaling burst nodes to 0...");
                let _ = nodes::scale_node_group(ng, 0);
            }

            // 6. Wait for gateway/webhook to stabilize at 1/1
            output::print_action("Waiting for infrastructure to stabilize (30s)...");
            std::thread::sleep(std::time::Duration::from_secs(30));

            // 7. Verify starting conditions
            let gw = kubectl.run(&["-n", &cfg.injection_namespace, "get", "deployment", &cfg.gateway_deployment, "-o", "jsonpath={.status.readyReplicas}/{.spec.replicas}"]).unwrap_or_default();
            let wh = kubectl.run(&["-n", &cfg.injection_namespace, "get", "deployment", &cfg.webhook_deployment, "-o", "jsonpath={.status.readyReplicas}/{.spec.replicas}"]).unwrap_or_default();
            let pods = drain::count_pods(&kubectl, &cfg.namespace, &app_label).unwrap_or(0);
            let burst_nodes = if let Some(ng) = &cfg.node_group {
                nodes::get_node_group_status(ng).map(|(d,_,_,_)| d).unwrap_or(0)
            } else { 0 };

            output::print_reset_verification(&gw, &wh, pods, burst_nodes);
        }

        Commands::Report { json } => {
            output::print_banner("Report");
            let conf = cfg.confluence.as_ref().ok_or_else(|| {
                anyhow::anyhow!(
                    "No confluence section configured. Add confluence to your burst-forge.yaml config."
                )
            })?;

            let data = std::fs::read_to_string(&json)
                .map_err(|e| anyhow::anyhow!("Failed to read results file '{json}': {e}"))?;
            let matrix_report: types::MatrixReport = serde_json::from_str(&data)
                .map_err(|e| anyhow::anyhow!("Failed to parse results JSON from '{json}': {e}"))?;

            publish_report(conf, &matrix_report, &cfg)?;
        }

        Commands::Nodes { action } => {
            output::print_banner("Nodes");
            let ng = cfg.node_group.as_ref().ok_or_else(|| {
                anyhow::anyhow!(
                    "No node_group configured. Add node_group section to burst-forge.yaml"
                )
            })?;

            match action {
                NodesAction::Up { count } => {
                    nodes::scale_node_group(ng, count)?;
                    nodes::wait_for_nodes(
                        &kubectl,
                        count,
                        std::time::Duration::from_secs(cfg.timeout_secs),
                        std::time::Duration::from_secs(cfg.node_poll_interval_secs),
                    )?;
                    nodes::tag_nodes(&kubectl, "burst-forge=true")?;
                    output::print_status(&format!(
                        "Node group scaled to {count} nodes {}",
                        output::bold_green("OK")
                    ));
                }
                NodesAction::Down => {
                    nodes::scale_node_group(ng, 0)?;
                    output::print_status(&format!(
                        "Node group {} scaling to 0",
                        ng.nodegroup_name
                    ));
                }
                NodesAction::Status => {
                    let (desired, min, max, status) =
                        nodes::get_node_group_status(ng)?;
                    let ready = nodes::count_ready_nodes(&kubectl)?;
                    output::print_node_status(
                        &ng.nodegroup_name,
                        &ng.cluster_name,
                        &ng.region,
                        &status,
                        min, desired, max, ready,
                    );
                }
            }
        }

        Commands::Flow {
            name: _,
            scenario,
            skip_scaling,
        } => {
            // Config already resolved via flow name → configs/{name}.yaml
            output::print_banner(&format!("Flow: {}", config_path.as_deref().unwrap_or("?")));
            let matrix_report =
                matrix::run_matrix(&kubectl, &cfg, scenario.as_deref(), skip_scaling)?;

            output::print_phase("Matrix Report");
            println!("{}", serde_json::to_string_pretty(&matrix_report)?);

            if let Some(conf) = &cfg.confluence {
                publish_report(conf, &matrix_report, &cfg)?;
            }
        }
    }

    Ok(())
}

/// List available flow configs from `configs/` directory.
fn list_flows() -> anyhow::Result<String> {
    let entries = std::fs::read_dir("configs")?;
    let names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.strip_suffix(".yaml").map(String::from)
        })
        .collect();
    Ok(names.join(", "))
}

/// Generate and publish a report to Confluence.
///
/// # Errors
///
/// Returns an error if publishing to Confluence fails.
fn publish_report(
    conf: &config::ConfluenceConfig,
    matrix_report: &types::MatrixReport,
    cfg: &config::Config,
) -> anyhow::Result<()> {
    let (title, content) = report::generate_report(matrix_report, cfg);
    output::print_publish_header();
    let url = report::publish_to_confluence(conf, &title, &content)
        .map_err(|e| anyhow::anyhow!("FATAL: Failed to publish results to Confluence: {e}"))?;
    output::print_publish_result(&url);
    Ok(())
}
