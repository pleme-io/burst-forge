//! EKS node group management for burst testing.
//!
//! Uses `aws` CLI subprocess calls (same pattern as kubectl) to manage
//! EKS managed node groups — scale up before burst tests, scale down after.

use std::process::Command;
use std::time::{Duration, Instant};

use crate::config::{NodeGroupConfig, WorkerNodeGroupConfig};
use crate::kubectl::KubeCtl;
use crate::output;

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

    output::print_action(&format!(
        "Scaling node group {} to {desired} nodes...",
        config.nodegroup_name
    ));

    let scaling_config = format!(
        "minSize={min},maxSize={max},desiredSize={desired}",
        min = u32::from(desired != 0),
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

    output::print_status(&format!("Node group scaling request accepted (desired={desired})"));
    Ok(())
}

/// Wait until the desired number of nodes are Ready AND schedulable.
///
/// Polls `kubectl get nodes` at the given interval until the desired count
/// of Ready+Schedulable nodes is reached or the timeout expires.
/// Nodes with `SchedulingDisabled` (draining) are not counted.
///
/// # Errors
///
/// Returns an error if the timeout is exceeded before nodes are ready.
pub fn wait_for_nodes(
    kubectl: &KubeCtl,
    desired: u32,
    timeout: Duration,
    poll_interval: Duration,
) -> anyhow::Result<()> {
    output::print_status(&format!(
        "Waiting for {desired} nodes to be Ready+Schedulable (timeout: {}s)...",
        timeout.as_secs()
    ));

    let start = Instant::now();

    loop {
        if start.elapsed() > timeout {
            let ready = count_ready_schedulable_nodes(kubectl).unwrap_or(0);
            anyhow::bail!(
                "Timeout waiting for {desired} Ready+Schedulable nodes after {}s (currently {ready} ready+schedulable)",
                timeout.as_secs()
            );
        }

        let ready = count_ready_schedulable_nodes(kubectl)?;
        let elapsed = start.elapsed().as_secs();
        output::print_progress(elapsed, &format!("Ready+Schedulable nodes: {ready}/{desired}"));

        if ready >= desired {
            output::print_status(&format!("Nodes: {ready}/{desired} Ready+Schedulable"));
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
    let base = replicas.div_ceil(pods_per_node);
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
    output::print_action(&format!("Labeling nodes with {label}..."));
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
/// Excludes nodes that are `NotReady` or have `SchedulingDisabled` (draining).
///
/// # Errors
///
/// Returns an error if the kubectl command fails.
pub fn count_ready_nodes(kubectl: &KubeCtl) -> anyhow::Result<u32> {
    count_ready_schedulable_nodes(kubectl)
}

/// Get the current number of Ready AND schedulable nodes.
///
/// A node showing `Ready,SchedulingDisabled` is draining and is excluded.
///
/// # Errors
///
/// Returns an error if the kubectl command fails.
pub fn count_ready_schedulable_nodes(kubectl: &KubeCtl) -> anyhow::Result<u32> {
    let output = kubectl.run(&["get", "nodes", "--no-headers"])?;
    #[allow(clippy::cast_possible_truncation)]
    let ready = output
        .lines()
        .filter(|line| {
            line.contains("Ready")
                && !line.contains("NotReady")
                && !line.contains("SchedulingDisabled")
        })
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

/// Wait for an image-warmup `DaemonSet` to be fully rolled out.
///
/// Matches the `DaemonSet` ready count against the number of Ready+schedulable
/// nodes. Polls until all desired pods are ready on every schedulable node.
///
/// # Errors
///
/// Returns an error if the timeout expires or kubectl fails.
#[allow(dead_code)]
pub fn wait_for_daemonset_ready(
    kubectl: &KubeCtl,
    namespace: &str,
    name: &str,
    timeout: Duration,
) -> anyhow::Result<()> {
    let schedulable = count_ready_schedulable_nodes(kubectl)?;
    output::print_status(&format!(
        "Waiting for DaemonSet {namespace}/{name} rollout on {schedulable} schedulable nodes (timeout: {}s)...",
        timeout.as_secs()
    ));

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
                output::print_progress(elapsed, &format!(
                    "DaemonSet {name}: {ready}/{desired} ready, {schedulable} schedulable nodes"
                ));

                #[allow(clippy::cast_possible_truncation)]
                if ready >= u64::from(schedulable)
                    && desired >= u64::from(schedulable)
                    && schedulable > 0
                {
                    output::print_status(&format!(
                        "Warmup: {ready}/{desired} pods on {schedulable} schedulable nodes"
                    ));
                    return Ok(());
                }
            }
            Err(e) => {
                let elapsed = start.elapsed().as_secs();
                output::print_progress(elapsed, &format!("DaemonSet {name}: not found yet ({e})"));
            }
        }

        std::thread::sleep(poll_interval);
    }
}

/// Wait for an EKS nodegroup to reach a target status (e.g., "ACTIVE").
/// Polls every 15s. Prevents `ResourceInUseException` from concurrent updates.
///
/// # Errors
///
/// Returns an error if the timeout is exceeded.
pub fn wait_for_nodegroup_status(
    cluster_name: &str,
    nodegroup_name: &str,
    region: &str,
    profile: Option<&str>,
    target_status: &str,
    timeout: Duration,
) -> anyhow::Result<()> {
    let start = Instant::now();
    let poll = Duration::from_secs(15);

    loop {
        let mut cmd = Command::new("aws");
        cmd.args([
            "eks", "describe-nodegroup",
            "--cluster-name", cluster_name,
            "--nodegroup-name", nodegroup_name,
            "--region", region,
            "--query", "nodegroup.status",
            "--output", "text",
        ]);
        if let Some(p) = profile {
            cmd.args(["--profile", p]);
        }

        let output = cmd.output();
        if let Ok(out) = output {
            let status = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if status == target_status {
                return Ok(());
            }
            if start.elapsed() > timeout {
                anyhow::bail!(
                    "Nodegroup {nodegroup_name} did not reach {target_status} (current: {status}) after {}s",
                    timeout.as_secs()
                );
            }
            #[allow(clippy::cast_possible_truncation)]
            let elapsed = start.elapsed().as_secs();
            output::print_progress(elapsed, &format!(
                "Nodegroup {nodegroup_name}: {status} (waiting for {target_status})"
            ));
        }

        std::thread::sleep(poll);
    }
}

/// Scale the worker node group to the desired count.
/// Waits for ACTIVE status before attempting the scale to avoid `ResourceInUseException`.
///
/// # Errors
///
/// Returns an error if the AWS API call fails.
pub fn scale_worker_group(config: &WorkerNodeGroupConfig, desired: u32) -> anyhow::Result<()> {
    // Wait for nodegroup to be ACTIVE before scaling (prevents ResourceInUseException)
    wait_for_nodegroup_status(
        &config.cluster_name,
        &config.nodegroup_name,
        &config.region,
        config.aws_profile.as_deref(),
        "ACTIVE",
        Duration::from_secs(300),
    )?;

    output::print_action(&format!(
        "Scaling worker group {} to {desired} nodes...",
        config.nodegroup_name
    ));

    let max = config.max_nodes;
    let scaling_config = format!(
        "minSize={min},maxSize={max},desiredSize={desired}",
        min = desired.min(3),
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
            "aws eks update-nodegroup-config (workers) failed: {}",
            stderr.trim()
        );
    }

    output::print_status(&format!("Worker group scaling to {desired}"));
    Ok(())
}

/// Count nodes belonging to a specific nodegroup by label.
///
/// # Errors
///
/// Returns an error if kubectl fails.
pub fn count_nodes_by_nodegroup(kubectl: &KubeCtl, nodegroup_name: &str) -> anyhow::Result<u32> {
    let label = format!("eks.amazonaws.com/nodegroup={nodegroup_name}");
    let output = kubectl.run(&["get", "nodes", "-l", &label, "--no-headers"])?;
    if output.is_empty() {
        return Ok(0);
    }
    #[allow(clippy::cast_possible_truncation)]
    Ok(output.lines().count() as u32)
}

/// Wait until burst nodes for a specific nodegroup reach exactly 0.
/// Filters by nodegroup label instead of counting all nodes.
///
/// # Errors
///
/// Returns an error if the timeout is exceeded.
pub fn wait_for_zero_burst_nodes(
    kubectl: &KubeCtl,
    nodegroup_name: &str,
    timeout: Duration,
) -> anyhow::Result<()> {
    let start = Instant::now();
    let poll = Duration::from_secs(15);

    loop {
        let count = count_nodes_by_nodegroup(kubectl, nodegroup_name).unwrap_or(u32::MAX);
        if count == 0 {
            output::print_status("Burst nodes at 0 (confirmed)");
            return Ok(());
        }

        if start.elapsed() > timeout {
            anyhow::bail!(
                "Burst node teardown incomplete: {count} nodes in {nodegroup_name} after {}s",
                timeout.as_secs()
            );
        }

        #[allow(clippy::cast_possible_truncation)]
        let elapsed = start.elapsed().as_secs();
        output::print_progress(elapsed, &format!("Waiting for {nodegroup_name}: {count} remaining"));
        std::thread::sleep(poll);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calculate_nodes_normal() {
        assert_eq!(calculate_nodes(100, 58), 3);
    }

    #[test]
    fn calculate_nodes_exact_fit() {
        assert_eq!(calculate_nodes(58, 58), 2);
    }

    #[test]
    fn calculate_nodes_zero_pods_per_node() {
        assert_eq!(calculate_nodes(100, 0), 1);
    }

    #[test]
    fn calculate_nodes_zero_replicas() {
        assert_eq!(calculate_nodes(0, 58), 1);
    }

    #[test]
    fn calculate_nodes_one_replica() {
        assert_eq!(calculate_nodes(1, 58), 2);
    }

    #[test]
    fn calculate_nodes_large_scale() {
        assert_eq!(calculate_nodes(1000, 58), 19);
    }

    #[test]
    fn calculate_nodes_small_pods_per_node() {
        assert_eq!(calculate_nodes(10, 1), 11);
    }

    #[test]
    fn calculate_nodes_one_pod_per_node() {
        assert_eq!(calculate_nodes(1, 1), 2);
    }
}
