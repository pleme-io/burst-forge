//! Shikumi-powered YAML configuration.

use serde::{Deserialize, Serialize};
use shikumi::{ConfigDiscovery, ConfigStore, Format};

/// A scaling scenario in the matrix.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scenario {
    pub name: String,
    #[serde(default = "default_replicas")]
    pub replicas: u32,
    #[serde(default = "default_one")]
    pub gateway_replicas: u32,
    #[serde(default = "default_one")]
    pub webhook_replicas: u32,
    /// Override node count (auto-calculated from replicas/pods_per_node if absent)
    #[serde(default)]
    pub nodes: Option<u32>,
}

/// EKS node group configuration for burst testing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeGroupConfig {
    /// EKS cluster name
    pub cluster_name: String,
    /// Node group name to scale
    pub nodegroup_name: String,
    /// AWS region
    #[serde(default = "default_region")]
    pub region: String,
    /// AWS profile
    #[serde(default)]
    pub aws_profile: Option<String>,
    /// Max pods per node (for calculating required nodes)
    #[serde(default = "default_pods_per_node")]
    pub pods_per_node: u32,
    /// Max node group size
    #[serde(default = "default_max_nodes")]
    pub max_nodes: u32,
}

/// Top-level burst-forge configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default = "default_namespace")]
    pub namespace: String,

    #[serde(default = "default_deployment")]
    pub deployment: String,

    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,

    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,

    #[serde(default)]
    pub cache_registry: Option<String>,

    #[serde(default)]
    pub required_images: Vec<String>,

    #[serde(default)]
    pub flux_kustomizations: Vec<String>,

    #[serde(default)]
    pub scenarios: Vec<Scenario>,

    /// EKS node group management for burst testing
    #[serde(default)]
    pub node_group: Option<NodeGroupConfig>,

    /// Namespace where Akeyless gateway and webhook live.
    #[serde(default = "default_akeyless_namespace")]
    pub akeyless_namespace: String,

    /// Label selector for gateway pods.
    #[serde(default = "default_gateway_label")]
    pub gateway_label: String,

    /// Label selector for webhook pods.
    #[serde(default = "default_webhook_label")]
    pub webhook_label: String,

    /// `HelmRelease` name for the gateway.
    #[serde(default = "default_gateway_release")]
    pub gateway_release: String,

    /// `HelmRelease` name for the webhook.
    #[serde(default = "default_webhook_release")]
    pub webhook_release: String,

    /// Injection detection mode: "sidecar" (2+ containers) or "env" (AKEYLESS_* env vars).
    #[serde(default = "default_injection_mode")]
    pub injection_mode: InjectionMode,
}

/// How burst-forge detects successful Akeyless secret injection.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum InjectionMode {
    /// Sidecar injection: 2+ containers indicates the Akeyless sidecar was injected.
    Sidecar,
    /// Environment-variable injection: `AKEYLESS_*` env vars present on any container.
    #[default]
    Env,
}

fn default_namespace() -> String { "scale-test".to_string() }
fn default_deployment() -> String { "nginx-burst".to_string() }
fn default_timeout() -> u64 { 600 }
fn default_poll_interval() -> u64 { 5 }
fn default_replicas() -> u32 { 50 }
fn default_one() -> u32 { 1 }
fn default_region() -> String { "us-east-1".to_string() }
fn default_pods_per_node() -> u32 { 58 }
fn default_max_nodes() -> u32 { 20 }
fn default_akeyless_namespace() -> String { "akeyless-system".to_string() }
fn default_gateway_label() -> String { "app.kubernetes.io/name=akeyless-api-gateway".to_string() }
fn default_webhook_label() -> String { "app=akeyless-secrets-injection".to_string() }
fn default_gateway_release() -> String { "akeyless-api-gateway".to_string() }
fn default_webhook_release() -> String { "akeyless-secrets-injection".to_string() }
fn default_injection_mode() -> InjectionMode { InjectionMode::Env }

/// Discover and load config via shikumi.
///
/// Resolution order:
/// 1. Explicit `--config` CLI path
/// 2. `BURST_FORGE_CONFIG` env var
/// 3. `~/.config/burst-forge/burst-forge.yaml`
/// 4. Defaults (no config file needed)
pub fn discover(explicit_path: Option<&str>) -> anyhow::Result<Config> {
    // If explicit path given, load via shikumi ConfigStore
    if let Some(path) = explicit_path {
        let store = ConfigStore::<Config>::load(path.as_ref(), "BURST_FORGE_")?;
        let guard = store.get();
        return Ok(Config::clone(&guard));
    }

    // Try shikumi discovery (env override + XDG paths)
    match ConfigDiscovery::new("burst-forge")
        .env_override("BURST_FORGE_CONFIG")
        .formats(&[Format::Yaml])
        .discover()
    {
        Ok(path) => {
            let store = ConfigStore::<Config>::load(&path, "BURST_FORGE_")?;
            let guard = store.get();
            Ok(Config::clone(&guard))
        }
        Err(_) => {
            // No config file found — deserialize from empty YAML so
            // serde #[default] functions produce the correct values
            // (Rust's Default trait gives empty strings, not our defaults).
            Ok(serde_json::from_str("{}")?)
        }
    }
}
