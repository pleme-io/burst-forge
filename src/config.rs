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
}

fn default_namespace() -> String { "scale-test".to_string() }
fn default_deployment() -> String { "nginx-burst".to_string() }
fn default_timeout() -> u64 { 600 }
fn default_poll_interval() -> u64 { 5 }
fn default_replicas() -> u32 { 50 }
fn default_one() -> u32 { 1 }
fn default_akeyless_namespace() -> String { "akeyless-system".to_string() }
fn default_gateway_label() -> String { "app.kubernetes.io/name=akeyless-api-gateway".to_string() }
fn default_webhook_label() -> String { "app=akeyless-secrets-injection".to_string() }
fn default_gateway_release() -> String { "akeyless-api-gateway".to_string() }
fn default_webhook_release() -> String { "akeyless-secrets-injection".to_string() }

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
            // No config file found — use defaults
            Ok(Config::default())
        }
    }
}
