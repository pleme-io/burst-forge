//! burst-forge — Kubernetes burst test orchestrator.
//!
//! Coordinated pod scaling with Akeyless injection verification.
//! Designed for scale testing: 0 → N pods with real secret injection,
//! measuring timing and scraping injection success/failure.

use std::process::Command;
use std::time::Instant;

use chrono::Utc;
use clap::{Parser, Subcommand};
use serde::Serialize;

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
    #[arg(long, env = "KUBECONFIG")]
    kubeconfig: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Verify infrastructure is ready for burst testing
    Verify {
        /// Namespace for workloads
        #[arg(long, default_value = "scale-test")]
        namespace: String,
    },

    /// Run a burst test: scale 0 → N, measure, report
    Burst {
        /// Number of replicas to burst to
        #[arg(long, default_value = "300")]
        replicas: u32,

        /// Deployment name
        #[arg(long, default_value = "nginx-burst")]
        deployment: String,

        /// Namespace
        #[arg(long, default_value = "scale-test")]
        namespace: String,

        /// Timeout in seconds
        #[arg(long, default_value = "600")]
        timeout: u64,

        /// Number of burst iterations
        #[arg(long, default_value = "1")]
        iterations: u32,
    },

    /// Reset deployment to 0 replicas
    Reset {
        #[arg(long, default_value = "nginx-burst")]
        deployment: String,

        #[arg(long, default_value = "scale-test")]
        namespace: String,
    },
}

#[derive(Serialize)]
struct BurstResult {
    timestamp: String,
    replicas_requested: u32,
    pods_running: u32,
    pods_failed: u32,
    pods_pending: u32,
    pods_with_sidecar: u32,
    injection_success_rate: f64,
    time_to_first_ready_ms: u64,
    time_to_all_ready_ms: Option<u64>,
    duration_ms: u64,
    nodes: u32,
    iteration: u32,
}

fn kubectl(args: &[&str], kubeconfig: Option<&str>) -> anyhow::Result<String> {
    let mut cmd = Command::new("kubectl");
    if let Some(kc) = kubeconfig {
        cmd.env("KUBECONFIG", kc);
    }
    cmd.args(args);
    let output = cmd.output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("kubectl failed: {}", stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn verify_infra(namespace: &str, kubeconfig: Option<&str>) -> anyhow::Result<()> {
    println!("Verifying infrastructure...");

    // Check nodes
    let nodes = kubectl(&["get", "nodes", "--no-headers"], kubeconfig)?;
    let node_count = nodes.lines().count();
    let ready_nodes = nodes
        .lines()
        .filter(|l| l.contains("Ready"))
        .count();
    println!("  Nodes: {ready_nodes}/{node_count} Ready");

    // Check Akeyless gateway
    let gw = kubectl(
        &["-n", "akeyless-system", "get", "pods", "-l", "app=akeyless-api-gateway", "--no-headers"],
        kubeconfig,
    )?;
    let gw_ready = gw.lines().filter(|l| l.contains("Running")).count();
    println!("  Akeyless gateway: {gw_ready} Running");

    // Check injection webhook
    let inj = kubectl(
        &["-n", "akeyless-system", "get", "pods", "-l", "app.kubernetes.io/name=akeyless-secrets-injection", "--no-headers"],
        kubeconfig,
    )?;
    let inj_ready = inj.lines().filter(|l| l.contains("Running")).count();
    println!("  Injection webhook: {inj_ready} Running");

    // Check deployment exists
    let dep = kubectl(
        &["-n", namespace, "get", "deployment", "nginx-burst", "--no-headers"],
        kubeconfig,
    );
    match dep {
        Ok(d) => println!("  Deployment: {d}"),
        Err(e) => println!("  Deployment: NOT FOUND ({e})"),
    }

    if gw_ready == 0 || inj_ready == 0 {
        anyhow::bail!("Infrastructure not ready: gateway={gw_ready}, webhook={inj_ready}");
    }

    println!("Infrastructure verified.");
    Ok(())
}

fn run_burst(
    replicas: u32,
    deployment: &str,
    namespace: &str,
    timeout: u64,
    iteration: u32,
    kubeconfig: Option<&str>,
) -> anyhow::Result<BurstResult> {
    let start = Instant::now();
    let timestamp = Utc::now().to_rfc3339();

    println!(
        "\n=== Burst #{iteration}: 0 → {replicas} replicas ===\n"
    );

    // Scale to 0 first (clean state)
    println!("  Resetting to 0...");
    kubectl(
        &["-n", namespace, "scale", "deployment", deployment, "--replicas=0"],
        kubeconfig,
    )?;

    // Wait for scale-down
    std::thread::sleep(std::time::Duration::from_secs(5));

    // BURST: Scale to N
    let burst_start = Instant::now();
    println!("  BURST: scaling to {replicas}...");
    kubectl(
        &[
            "-n", namespace, "scale", "deployment", deployment,
            &format!("--replicas={replicas}"),
        ],
        kubeconfig,
    )?;

    // Poll pod status until timeout
    let mut first_ready_time: Option<u64> = None;
    let poll_interval = std::time::Duration::from_secs(5);
    let timeout_duration = std::time::Duration::from_secs(timeout);

    loop {
        if burst_start.elapsed() > timeout_duration {
            println!("  TIMEOUT after {}s", timeout);
            break;
        }

        let pods_json = kubectl(
            &[
                "-n", namespace, "get", "pods", "-l", &format!("app={deployment}"),
                "-o", "json",
            ],
            kubeconfig,
        )?;

        let pods: serde_json::Value = serde_json::from_str(&pods_json)?;
        let empty = vec![];
        let items = pods["items"].as_array().unwrap_or(&empty);

        let mut running = 0u32;
        let mut pending = 0u32;
        let mut failed = 0u32;
        let mut with_sidecar = 0u32;

        for pod in items {
            let phase = pod["status"]["phase"].as_str().unwrap_or("");
            let containers = pod["spec"]["containers"].as_array().map(|c| c.len()).unwrap_or(0);

            match phase {
                "Running" => running += 1,
                "Pending" => pending += 1,
                "Failed" => failed += 1,
                _ => {}
            }

            // Check for Akeyless sidecar (2+ containers = injection happened)
            if containers >= 2 {
                with_sidecar += 1;
            }
        }

        let elapsed_ms = burst_start.elapsed().as_millis() as u64;

        if running > 0 && first_ready_time.is_none() {
            first_ready_time = Some(elapsed_ms);
        }

        print!(
            "\r  [{:>5}ms] Running: {running:>3} | Pending: {pending:>3} | Failed: {failed:>3} | Injected: {with_sidecar:>3}",
            elapsed_ms
        );

        if running >= replicas {
            println!("\n  ALL {replicas} PODS READY in {elapsed_ms}ms");
            return Ok(BurstResult {
                timestamp,
                replicas_requested: replicas,
                pods_running: running,
                pods_failed: failed,
                pods_pending: pending,
                pods_with_sidecar: with_sidecar,
                injection_success_rate: if running > 0 {
                    with_sidecar as f64 / running as f64 * 100.0
                } else {
                    0.0
                },
                time_to_first_ready_ms: first_ready_time.unwrap_or(0),
                time_to_all_ready_ms: Some(elapsed_ms),
                duration_ms: start.elapsed().as_millis() as u64,
                nodes: items.len() as u32, // approximate
                iteration,
            });
        }

        if running > 0 && pending == 0 && failed == 0 && running < replicas {
            // Stuck — all created pods are running but we can't create more
            println!("\n  CAPACITY LIMIT: {running}/{replicas} pods (no more schedulable)");
            break;
        }

        std::thread::sleep(poll_interval);
    }

    // Final count
    let pods_json = kubectl(
        &[
            "-n", namespace, "get", "pods", "-l", &format!("app={deployment}"),
            "-o", "json",
        ],
        kubeconfig,
    )?;
    let pods: serde_json::Value = serde_json::from_str(&pods_json)?;
    let empty = vec![];
        let items = pods["items"].as_array().unwrap_or(&empty);

    let mut running = 0u32;
    let mut pending = 0u32;
    let mut failed = 0u32;
    let mut with_sidecar = 0u32;

    for pod in items {
        let phase = pod["status"]["phase"].as_str().unwrap_or("");
        let containers = pod["spec"]["containers"].as_array().map(|c| c.len()).unwrap_or(0);
        match phase {
            "Running" => running += 1,
            "Pending" => pending += 1,
            _ => failed += 1,
        }
        if containers >= 2 {
            with_sidecar += 1;
        }
    }

    Ok(BurstResult {
        timestamp,
        replicas_requested: replicas,
        pods_running: running,
        pods_failed: failed,
        pods_pending: pending,
        pods_with_sidecar: with_sidecar,
        injection_success_rate: if running > 0 {
            with_sidecar as f64 / running as f64 * 100.0
        } else {
            0.0
        },
        time_to_first_ready_ms: first_ready_time.unwrap_or(0),
        time_to_all_ready_ms: None,
        duration_ms: start.elapsed().as_millis() as u64,
        nodes: 0,
        iteration,
    })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();
    let kubeconfig = cli.kubeconfig.as_deref();

    match cli.command {
        Commands::Verify { namespace } => {
            verify_infra(&namespace, kubeconfig)?;
        }

        Commands::Burst {
            replicas,
            deployment,
            namespace,
            timeout,
            iterations,
        } => {
            verify_infra(&namespace, kubeconfig)?;

            let mut results = Vec::new();
            for i in 1..=iterations {
                let result = run_burst(
                    replicas,
                    &deployment,
                    &namespace,
                    timeout,
                    i,
                    kubeconfig,
                )?;
                println!("\n--- Iteration {i} Results ---");
                println!("  Pods Running:     {}/{}", result.pods_running, result.replicas_requested);
                println!("  Injection Rate:   {:.1}%", result.injection_success_rate);
                println!("  First Ready:      {}ms", result.time_to_first_ready_ms);
                if let Some(all) = result.time_to_all_ready_ms {
                    println!("  All Ready:        {}ms", all);
                }
                println!("  Total Duration:   {}ms", result.duration_ms);
                results.push(result);

                if i < iterations {
                    println!("\n  Cooling down 10s...");
                    std::thread::sleep(std::time::Duration::from_secs(10));
                }
            }

            // Summary
            println!("\n=== BURST TEST SUMMARY ===");
            println!("{}", serde_json::to_string_pretty(&results)?);
        }

        Commands::Reset {
            deployment,
            namespace,
        } => {
            kubectl(
                &["-n", &namespace, "scale", "deployment", &deployment, "--replicas=0"],
                kubeconfig,
            )?;
            println!("Reset {deployment} to 0 replicas");
        }
    }

    Ok(())
}
