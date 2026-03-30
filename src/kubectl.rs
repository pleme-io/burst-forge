//! kubectl subprocess abstraction.

use std::process::Command;

/// Wrapper around the `kubectl` binary.
#[derive(Debug, Clone)]
pub struct KubeCtl {
    kubeconfig: Option<String>,
}

impl KubeCtl {
    /// Create a new `KubeCtl` with an optional kubeconfig path.
    #[must_use]
    pub fn new(kubeconfig: Option<String>) -> Self {
        Self { kubeconfig }
    }

    /// Run kubectl with the given arguments and return stdout.
    ///
    /// # Errors
    ///
    /// Returns an error if kubectl exits non-zero or cannot be spawned.
    pub fn run(&self, args: &[&str]) -> anyhow::Result<String> {
        let mut cmd = Command::new("kubectl");
        if let Some(kc) = &self.kubeconfig {
            cmd.arg("--kubeconfig").arg(kc);
        }
        cmd.args(args);
        let output = cmd.output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("kubectl failed: {}", stderr.trim());
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Run kubectl with `-o json` appended and parse the result.
    ///
    /// # Errors
    ///
    /// Returns an error if kubectl fails or the output is not valid JSON.
    pub fn get_json(&self, args: &[&str]) -> anyhow::Result<serde_json::Value> {
        let mut full_args: Vec<&str> = args.to_vec();
        full_args.push("-o");
        full_args.push("json");
        let output = self.run(&full_args)?;
        let value: serde_json::Value = serde_json::from_str(&output)?;
        Ok(value)
    }

    /// Patch a `HelmRelease` to set the desired replica count via values override.
    ///
    /// This patches the `HelmRelease` spec to set `spec.values.replicaCount`.
    ///
    /// # Errors
    ///
    /// Returns an error if the kubectl patch command fails.
    pub fn patch_helmrelease_replicas(
        &self,
        ns: &str,
        name: &str,
        replicas: u32,
    ) -> anyhow::Result<()> {
        let patch = format!(
            r#"{{"spec":{{"values":{{"replicaCount":{replicas}}}}}}}"#,
        );
        self.run(&[
            "-n",
            ns,
            "patch",
            "helmrelease.helm.toolkit.fluxcd.io",
            name,
            "--type=merge",
            "-p",
            &patch,
        ])?;
        Ok(())
    }
}
