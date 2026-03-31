//! EKS node group management for burst testing.
//!
//! Uses `aws` CLI subprocess calls (same pattern as kubectl) to manage
//! EKS managed node groups — scale up before burst tests, scale down after.

use std::process::Command;
use std::time::{Duration, Instant};

use crate::config::NodeGroupConfig;
use crate::kubectl::KubeCtl;

/// Scale an EKS managed node group to the desired size.
///
/// Calls `aws eks update-nodegroup-config` to set min/desired/max sizes.
/// The desired count is clamped to `config.max_nodes`.
///
/// # Errors
///
/// Returns an error if the AWS CLI command fails.
pub fn scale_node_group(config: &NodeGroupConfig, desired: u32) -> anyhow::Result<()> {
    let desired = desired.min(config.max_nodes);

    println!(
        "  Scaling node group {} to {desired} nodes...",
        config.nodegroup_name
    );

    let scaling_config = format!(
        "minSize={min},maxSize={max},desiredSize={desired}",
        min = if desired == 0 { 0 } else { 1 },
        max = config.max_nodes,
    );

    let mut cmd = Command::new("aws");
    cmd.args([
        "eks",
        "update-nodegroup-config",
        "--cluster-name",
        &config.cluster_name,
        "--nodegroup-name",
        &config.nodegroup_name,
        "--scaling-config",
        &scaling_config,
        "--region",
        &config.region,
    ]);

    if let Some(profile) = &config.aws_profile {
        cmd.args(["--profile", profile]);
    }

    let output = cmd.output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "aws eks update-nodegroup-config failed: {}",
            stderr.trim()
        );
    }

    println!("  Node group scaling request accepted (desired={desired})");
    Ok(())
}

/// Wait until the desired number of nodes are in Ready state.
///
/// Polls `kubectl get nodes` at 15-second intervals until the desired count
/// of Ready nodes is reached or the timeout expires.
///
/// # Errors
///
/// Returns an error if the timeout is exceeded before nodes are ready.
pub fn wait_for_nodes(
    kubectl: &KubeCtl,
    desired: u32,
    timeout: Duration,
) -> anyhow::Result<()> {
    println!("  Waiting for {desired} nodes to be Ready (timeout: {}s)...", timeout.as_secs());

    let start = Instant::now();
    let poll_interval = Duration::from_secs(15);

    loop {
        if start.elapsed() > timeout {
            anyhow::bail!(
                "Timeout waiting for {desired} Ready nodes after {}s",
                timeout.as_secs()
            );
        }

        let ready = count_ready_nodes(kubectl)?;
        let elapsed = start.elapsed().as_secs();
        println!("  [{elapsed:>4}s] Ready nodes: {ready}/{desired}");

        if ready >= desired {
            println!("  All {desired} nodes are Ready");
            return Ok(());
        }

        std::thread::sleep(poll_interval);
    }
}

/// Calculate the number of nodes needed for a given replica count.
///
/// Uses ceiling division plus one headroom node:
/// `ceil(replicas / pods_per_node) + 1`
#[must_use]
pub fn calculate_nodes(replicas: u32, pods_per_node: u32) -> u32 {
    if pods_per_node == 0 {
        return 1;
    }
    // Ceiling division
    let base = (replicas + pods_per_node - 1) / pods_per_node;
    // One headroom node
    base + 1
}

/// Label burst-forge-managed nodes with `burst-forge=true`.
///
/// Applies the label to all nodes that do not already have it.
///
/// # Errors
///
/// Returns an error if kubectl labeling fails.
pub fn tag_nodes(kubectl: &KubeCtl, label: &str) -> anyhow::Result<()> {
    println!("  Labeling nodes with {label}...");
    // Label all nodes; --overwrite avoids errors if already set
    kubectl.run(&[
        "label",
        "nodes",
        "--all",
        label,
        "--overwrite",
    ])?;
    Ok(())
}

/// Get the current number of Ready nodes in the cluster.
///
/// # Errors
///
/// Returns an error if the kubectl command fails.
pub fn count_ready_nodes(kubectl: &KubeCtl) -> anyhow::Result<u32> {
    let output = kubectl.run(&["get", "nodes", "--no-headers"])?;
    #[allow(clippy::cast_possible_truncation)]
    let ready = output
        .lines()
        .filter(|line| line.contains("Ready") && !line.contains("NotReady"))
        .count() as u32;
    Ok(ready)
}

/// Get the current node group status from EKS.
///
/// Returns `(desired_size, min_size, max_size, status)`.
///
/// # Errors
///
/// Returns an error if the AWS CLI command fails or output cannot be parsed.
pub fn get_node_group_status(
    config: &NodeGroupConfig,
) -> anyhow::Result<(u32, u32, u32, String)> {
    let mut cmd = Command::new("aws");
    cmd.args([
        "eks",
        "describe-nodegroup",
        "--cluster-name",
        &config.cluster_name,
        "--nodegroup-name",
        &config.nodegroup_name,
        "--region",
        &config.region,
        "--output",
        "json",
    ]);

    if let Some(profile) = &config.aws_profile {
        cmd.args(["--profile", profile]);
    }

    let output = cmd.output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("aws eks describe-nodegroup failed: {}", stderr.trim());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout)?;

    let scaling = &json["nodegroup"]["scalingConfig"];
    let desired = scaling["desiredSize"].as_u64().unwrap_or(0);
    let min = scaling["minSize"].as_u64().unwrap_or(0);
    let max = scaling["maxSize"].as_u64().unwrap_or(0);
    let status = json["nodegroup"]["status"]
        .as_str()
        .unwrap_or("UNKNOWN")
        .to_string();

    #[allow(clippy::cast_possible_truncation)]
    Ok((desired as u32, min as u32, max as u32, status))
}

/// Wait for an image-warmup DaemonSet to be fully rolled out.
///
/// Polls until all desired pods in the DaemonSet are ready, ensuring
/// images are pre-pulled on every node.
///
/// # Errors
///
/// Returns an error if the timeout expires or kubectl fails.
pub fn wait_for_daemonset_ready(
    kubectl: &KubeCtl,
    namespace: &str,
    name: &str,
    timeout: Duration,
) -> anyhow::Result<()> {
    println!("  Waiting for DaemonSet {namespace}/{name} rollout (timeout: {}s)...", timeout.as_secs());

    let start = Instant::now();
    let poll_interval = Duration::from_secs(15);

    loop {
        if start.elapsed() > timeout {
            anyhow::bail!(
                "Timeout waiting for DaemonSet {namespace}/{name} after {}s",
                timeout.as_secs()
            );
        }

        let result = kubectl.get_json(&[
            "-n",
            namespace,
            "get",
            "daemonset",
            name,
        ]);

        match result {
            Ok(json) => {
                let desired = json["status"]["desiredNumberScheduled"]
                    .as_u64()
                    .unwrap_or(0);
                let ready = json["status"]["numberReady"].as_u64().unwrap_or(0);
                let elapsed = start.elapsed().as_secs();
                println!("  [{elapsed:>4}s] DaemonSet {name}: {ready}/{desired} ready");

                if desired > 0 && ready >= desired {
                    println!("  DaemonSet {name} fully rolled out");
                    return Ok(());
                }
            }
            Err(e) => {
                let elapsed = start.elapsed().as_secs();
                println!("  [{elapsed:>4}s] DaemonSet {name}: not found yet ({e})");
            }
        }

        std::thread::sleep(poll_interval);
    }
}
