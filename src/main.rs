//! burst-forge -- Kubernetes burst test orchestrator.
//!
//! Coordinated pod scaling with Akeyless injection verification.
//! Designed for scale testing: 0 -> N pods with real secret injection,
//! measuring timing and scraping injection success/failure.

mod burst;
mod config;
mod flux;
mod kubectl;
mod matrix;
mod nodes;
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

    /// Manage EKS node group for burst testing
    Nodes {
        #[command(subcommand)]
        action: NodesAction,
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

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();
    let cfg = config::discover(cli.config.as_deref())?;
    let kubectl = KubeCtl::new(cli.kubeconfig);

    match cli.command {
        Commands::Verify => {
            let result = verify::verify_infra(&kubectl, &cfg)?;
            println!("\n{}", serde_json::to_string_pretty(&result)?);
        }

        Commands::Wait => {
            flux::wait_for_kustomizations(
                &kubectl,
                &cfg.flux_kustomizations,
                cfg.timeout_secs,
                cfg.poll_interval_secs,
            )?;
        }

        Commands::Burst {
            replicas,
            iterations,
        } => {
            let target_replicas = replicas.unwrap_or(50);

            verify::verify_infra(&kubectl, &cfg)?;

            let mut results = Vec::new();
            for i in 1..=iterations {
                let result = burst::run_burst(&kubectl, &cfg, target_replicas, i)?;

                println!("\n--- Iteration {i} Results ---");
                println!(
                    "  Pods Running:     {}/{}",
                    result.pods_running, result.replicas_requested
                );
                println!("  Injection Rate:   {:.1}%", result.injection_success_rate);
                println!("  First Ready:      {}ms", result.time_to_first_ready_ms);
                if let Some(all) = result.time_to_all_ready_ms {
                    println!("  All Ready:        {all}ms");
                }
                println!("  Total Duration:   {}ms", result.duration_ms);
                results.push(result);

                if i < iterations {
                    println!("\n  Cooling down 10s...");
                    std::thread::sleep(std::time::Duration::from_secs(10));
                }
            }

            println!("\n=== BURST TEST SUMMARY ===");
            println!("{}", serde_json::to_string_pretty(&results)?);
        }

        Commands::Matrix {
            scenario,
            skip_scaling,
        } => {
            let report =
                matrix::run_matrix(&kubectl, &cfg, scenario.as_deref(), skip_scaling)?;
            println!("\n=== MATRIX REPORT ===");
            println!("{}", serde_json::to_string_pretty(&report)?);
        }

        Commands::Reset => {
            kubectl.run(&[
                "-n",
                &cfg.namespace,
                "scale",
                "deployment",
                &cfg.deployment,
                "--replicas=0",
            ])?;
            println!("Reset {} to 0 replicas", cfg.deployment);
        }

        Commands::Nodes { action } => {
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
                    )?;
                    nodes::tag_nodes(&kubectl, "burst-forge=true")?;
                    println!("Node group scaled to {count} nodes");
                }
                NodesAction::Down => {
                    nodes::scale_node_group(ng, 0)?;
                    println!(
                        "Node group {} scaling to 0",
                        ng.nodegroup_name
                    );
                }
                NodesAction::Status => {
                    let (desired, min, max, status) =
                        nodes::get_node_group_status(ng)?;
                    let ready = nodes::count_ready_nodes(&kubectl)?;
                    println!("Node Group: {}", ng.nodegroup_name);
                    println!("  Cluster:  {}", ng.cluster_name);
                    println!("  Region:   {}", ng.region);
                    println!("  Status:   {status}");
                    println!("  Scaling:  min={min} desired={desired} max={max}");
                    println!("  Ready:    {ready} nodes in cluster");
                }
            }
        }
    }

    Ok(())
}
