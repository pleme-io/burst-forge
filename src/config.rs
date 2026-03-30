//! Shikumi-style YAML configuration.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

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
#[derive(Debug, Clone, Serialize, Deserialize)]
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

fn default_namespace() -> String {
    "scale-test".to_string()
}

fn default_deployment() -> String {
    "nginx-burst".to_string()
}

fn default_timeout() -> u64 {
    600
}

fn default_poll_interval() -> u64 {
    5
}

fn default_replicas() -> u32 {
    50
}

fn default_one() -> u32 {
    1
}

fn default_akeyless_namespace() -> String {
    "akeyless-system".to_string()
}

fn default_gateway_label() -> String {
    "app.kubernetes.io/name=akeyless-api-gateway".to_string()
}

fn default_webhook_label() -> String {
    "app=akeyless-secrets-injection".to_string()
}

fn default_gateway_release() -> String {
    "akeyless-api-gateway".to_string()
}

fn default_webhook_release() -> String {
    "akeyless-secrets-injection".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            namespace: default_namespace(),
            deployment: default_deployment(),
            timeout_secs: default_timeout(),
            poll_interval_secs: default_poll_interval(),
            cache_registry: None,
            required_images: Vec::new(),
            flux_kustomizations: Vec::new(),
            scenarios: Vec::new(),
            akeyless_namespace: default_akeyless_namespace(),
            gateway_label: default_gateway_label(),
            webhook_label: default_webhook_label(),
            gateway_release: default_gateway_release(),
            webhook_release: default_webhook_release(),
        }
    }
}

/// Discover and load the configuration file.
///
/// Resolution order:
/// 1. Explicit `--config` CLI path
/// 2. `BURST_FORGE_CONFIG` environment variable
/// 3. `~/.config/burst-forge/burst-forge.yaml`
/// 4. Defaults
///
/// # Errors
///
/// Returns an error if a specified config file cannot be read or parsed.
pub fn discover(explicit_path: Option<&str>) -> anyhow::Result<Config> {
    // 1. Explicit path
    if let Some(path) = explicit_path {
        let contents = std::fs::read_to_string(path)?;
        let config: Config = serde_yaml_ng::from_str(&contents)?;
        return Ok(config);
    }

    // 2. Environment variable
    if let Ok(path) = std::env::var("BURST_FORGE_CONFIG")
        && let Ok(contents) = std::fs::read_to_string(&path)
    {
        let config: Config = serde_yaml_ng::from_str(&contents)?;
        return Ok(config);
    }

    // 3. XDG-style config path
    if let Some(config_dir) = dirs::config_dir() {
        let path = config_dir.join("burst-forge").join("burst-forge.yaml");
        if path.exists() {
            let contents = std::fs::read_to_string(&path)?;
            let config: Config = serde_yaml_ng::from_str(&contents)?;
            return Ok(config);
        }
    }

    // 4. Also check ~/.config directly (Linux convention)
    let home_config = home_config_path();
    if home_config.exists() {
        let contents = std::fs::read_to_string(&home_config)?;
        let config: Config = serde_yaml_ng::from_str(&contents)?;
        return Ok(config);
    }

    // 5. Defaults
    Ok(Config::default())
}

fn home_config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("burst-forge")
        .join("burst-forge.yaml")
}
