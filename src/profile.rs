//! Customer profile — maps a customer's Akeyless environment to test parameters.
//!
//! Profiles are portable descriptions of WHAT a customer needs. They are separate
//! from cluster bindings (WHERE to test) and experiment configs (HOW to test).

use serde::{Deserialize, Serialize};

/// Top-level customer profile loaded from YAML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomerProfile {
    pub customer: CustomerInfo,
    pub environment: EnvironmentSpec,
    pub workload: WorkloadSpec,
    pub akeyless: AkeylessSpec,
    #[serde(default)]
    pub constraints: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomerInfo {
    pub name: String,
    #[serde(default)]
    pub ticket: Option<String>,
    #[serde(default)]
    pub contacts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentSpec {
    pub nodes: u32,
    #[serde(default)]
    pub node_type: Option<String>,
    #[serde(default)]
    pub node_memory_gb: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkloadSpec {
    /// What the customer actually needs
    pub target_pods: u32,
    /// What we test to (typically 2-3x target for headroom validation)
    #[serde(default = "default_test_max")]
    pub test_max_pods: u32,
    #[serde(default = "default_secrets")]
    pub secrets_per_pod: u32,
    #[serde(default = "default_init")]
    pub init_containers: u32,
    #[serde(default)]
    pub workload_kind: WorkloadKind,
    #[serde(default)]
    pub restart_policy: Option<String>,
    #[serde(default)]
    pub pod_memory_gb: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkloadKind {
    #[default]
    Deployment,
    Job,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AkeylessSpec {
    #[serde(default = "default_auth")]
    pub auth_method: String,
    #[serde(default = "default_gw_nodes")]
    pub gateway_nodes: GatewayNodeMode,
    #[serde(default)]
    pub gateway_headroom_pct: Option<u32>,
    #[serde(default = "default_gw_memory")]
    pub gateway_memory: String,
    #[serde(default = "default_wh_timeout")]
    pub webhook_timeout_secs: u32,
    #[serde(default = "default_qps")]
    pub qps: u32,
    #[serde(default = "default_burst_qps")]
    pub burst_qps: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GatewayNodeMode {
    #[default]
    Shared,
    Dedicated,
}

fn default_test_max() -> u32 { 1000 }
fn default_secrets() -> u32 { 2 }
fn default_init() -> u32 { 1 }
fn default_auth() -> String { "k8s".to_string() }
fn default_gw_nodes() -> GatewayNodeMode { GatewayNodeMode::Shared }
fn default_gw_memory() -> String { "512Mi".to_string() }
fn default_wh_timeout() -> u32 { 30 }
fn default_qps() -> u32 { 5 }
fn default_burst_qps() -> u32 { 10 }

impl CustomerProfile {
    /// Load from a YAML file path.
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Failed to read profile {path}: {e}"))?;
        let profile: Self = serde_yaml::from_str(&content)
            .map_err(|e| anyhow::anyhow!("Failed to parse profile {path}: {e}"))?;
        profile.validate()?;
        Ok(profile)
    }

    /// Validate the profile constraints.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.workload.target_pods == 0 {
            anyhow::bail!("target_pods must be > 0");
        }
        if self.workload.test_max_pods < self.workload.target_pods {
            anyhow::bail!("test_max_pods must be >= target_pods");
        }
        if self.akeyless.qps == 0 {
            anyhow::bail!("qps must be > 0");
        }
        Ok(())
    }

    /// Calculate theoretical minimum injection time in seconds.
    pub fn theoretical_minimum_secs(&self, gw_replicas: u32) -> f64 {
        let total_requests = self.workload.test_max_pods * self.workload.secrets_per_pod;
        let aggregate_qps = gw_replicas * self.akeyless.qps;
        total_requests as f64 / aggregate_qps as f64
    }
}
