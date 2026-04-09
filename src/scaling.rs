//! Infrastructure deployment scaling strategies.
//!
//! Encapsulates how to scale and clean up infrastructure deployments,
//! supporting suspend-and-scale, HelmRelease patching, and direct kubectl scale.

use std::time::Duration;

use crate::config::{InfraDeployment, ScalingStrategy};
use crate::kubectl::KubeCtl;
use crate::output;

/// Scale an infrastructure deployment to the target replica count.
///
/// # Errors
///
/// Returns an error if kubectl commands fail.
pub fn scale_deployment(
    kubectl: &KubeCtl,
    deployment: &InfraDeployment,
    target_replicas: u32,
    rollout_wait_secs: u64,
) -> anyhow::Result<()> {
    // Step 1: Suspend HelmRelease if needed
    if deployment.scaling_strategy == ScalingStrategy::SuspendAndScale
        && !deployment.helmrelease.is_empty()
    {
        output::print_action(&format!("  Suspending {} HelmRelease...", deployment.name));
        let _ = kubectl.run(&[
            "-n", &deployment.namespace, "patch",
            "helmrelease.helm.toolkit.fluxcd.io", &deployment.helmrelease,
            "--type=merge", "-p", r#"{"spec":{"suspend":true}}"#,
        ]);
    }

    // Step 2: Scale node group if configured
    if let Some(ng) = &deployment.node_group {
        let needed = ng.desired_for_pods(target_replicas);
        output::print_action(&format!(
            "  {} nodes: {} pods × {} pods/node => {needed} nodes",
            deployment.name, target_replicas, ng.pods_per_node
        ));
        crate::nodes::scale_infra_node_group(
            kubectl,
            ng,
            needed,
            &deployment.name,
            Duration::from_secs(rollout_wait_secs),
        )?;
    }

    // Step 3: Scale deployment (with batching if configured)
    let batch = deployment.batch_size;
    if batch > 0 && target_replicas > batch {
        let mut current = 0u32;
        let mut wave = 0u32;
        while current < target_replicas {
            let next = (current + batch).min(target_replicas);
            wave += 1;
            output::print_action(&format!(
                "  {} wave {wave}: {current} -> {next} / {target_replicas} replicas...",
                deployment.name
            ));
            do_scale(kubectl, deployment, next)?;
            let deploy_path = format!("deployment/{}", deployment.deployment);
            if let Err(e) = kubectl.run(&[
                "-n", &deployment.namespace, "rollout", "status",
                &deploy_path, &format!("--timeout={rollout_wait_secs}s"),
            ]) {
                output::print_warning(&format!(
                    "{} wave {wave} rollout wait: {e}", deployment.name
                ));
            }
            current = next;
        }
    } else {
        output::print_action(&format!(
            "  {} -> {target_replicas} replicas...", deployment.name
        ));
        do_scale(kubectl, deployment, target_replicas)?;
        let deploy_path = format!("deployment/{}", deployment.deployment);
        let _ = kubectl.run(&[
            "-n", &deployment.namespace, "rollout", "status",
            &deploy_path, &format!("--timeout={rollout_wait_secs}s"),
        ]);
    }

    // Post-scale stabilization — let pods fully warm up before the next
    // deployment starts. Critical for GW→WH ordering: GW JVMs need time
    // to finish initialization even after passing readiness probe.
    if deployment.post_scale_stabilize_secs > 0 {
        output::print_action(&format!(
            "  {} stabilization: waiting {}s...",
            deployment.name, deployment.post_scale_stabilize_secs
        ));
        std::thread::sleep(Duration::from_secs(deployment.post_scale_stabilize_secs));
    }

    Ok(())
}

/// Resume/cleanup an infrastructure deployment after the experiment.
pub fn cleanup_deployment(kubectl: &KubeCtl, deployment: &InfraDeployment) {
    // Scale to 1 (safe baseline)
    let _ = kubectl.run(&[
        "-n", &deployment.namespace, "scale", "deployment",
        &deployment.deployment, "--replicas=1",
    ]);

    // Resume HelmRelease if it was suspended
    if !deployment.helmrelease.is_empty() {
        let _ = kubectl.run(&[
            "-n", &deployment.namespace, "patch",
            "helmrelease.helm.toolkit.fluxcd.io", &deployment.helmrelease,
            "--type=merge", "-p", r#"{"spec":{"suspend":false}}"#,
        ]);
    }
}

/// Execute the actual scale command based on strategy.
fn do_scale(kubectl: &KubeCtl, deployment: &InfraDeployment, replicas: u32) -> anyhow::Result<()> {
    match deployment.scaling_strategy {
        ScalingStrategy::SuspendAndScale | ScalingStrategy::DirectScale => {
            kubectl.run(&[
                "-n", &deployment.namespace, "scale", "deployment",
                &deployment.deployment,
                &format!("--replicas={replicas}"),
            ])?;
        }
        ScalingStrategy::HelmreleasePatch => {
            if !deployment.replica_patch.is_empty() {
                let patch = deployment.replica_patch.replace("{replicas}", &replicas.to_string());
                kubectl.run(&[
                    "-n", &deployment.namespace, "patch",
                    "helmrelease.helm.toolkit.fluxcd.io", &deployment.helmrelease,
                    "--type=merge", "-p", &patch,
                ])?;
            }
        }
    }
    Ok(())
}
