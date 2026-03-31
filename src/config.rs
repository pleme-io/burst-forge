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
    /// Override node count (auto-calculated from `replicas/pods_per_node` if absent)
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

/// Image cache configuration for Zot registry lookups.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageCacheConfig {
    /// Namespace where image cache (Zot) pods run.
    #[serde(default = "default_image_cache_namespace")]
    pub namespace: String,

    /// Label selector for Zot pods.
    #[serde(default = "default_image_cache_label")]
    pub label: String,

    /// Registry URL inside the cluster.
    pub registry: String,
}

/// `DaemonSet` warmup configuration for image pre-pull.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WarmupConfig {
    /// Namespace where the warmup `DaemonSet` runs.
    pub namespace: String,

    /// `DaemonSet` name.
    pub name: String,

    /// Timeout in seconds to wait for warmup rollout.
    #[serde(default = "default_warmup_timeout")]
    pub timeout_secs: u64,
}

/// `FluxCD` kustomization configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FluxConfig {
    /// Namespace where `FluxCD` kustomizations live.
    #[serde(default = "default_flux_namespace")]
    pub namespace: String,

    /// Kustomization names to wait for.
    #[serde(default)]
    pub kustomizations: Vec<String>,
}

/// Top-level burst-forge configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default = "default_namespace")]
    pub namespace: String,

    #[serde(default = "default_deployment")]
    pub deployment: String,

    /// Label selector for burst test pods (default: `app={deployment}`).
    #[serde(default)]
    pub pod_label: Option<String>,

    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,

    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,

    /// Cooldown seconds between scenarios in the matrix (applied AFTER drain completes).
    #[serde(default = "default_cooldown")]
    pub cooldown_secs: u64,

    /// Seconds to wait for `HelmRelease` rollout after patching replicas.
    #[serde(default = "default_rollout_wait")]
    pub rollout_wait_secs: u64,

    /// Maximum seconds to wait for pods to terminate during drain.
    #[serde(default = "default_drain_timeout")]
    pub drain_timeout_secs: u64,

    /// How often (seconds) to poll pod count during drain.
    #[serde(default = "default_drain_poll_interval")]
    pub drain_poll_interval_secs: u64,

    /// Node readiness poll interval in seconds.
    #[serde(default = "default_node_poll_interval")]
    pub node_poll_interval_secs: u64,

    /// Image cache (Zot registry) configuration.
    #[serde(default)]
    pub image_cache: Option<ImageCacheConfig>,

    #[serde(default)]
    pub required_images: Vec<String>,

    /// `FluxCD` kustomization wait configuration.
    #[serde(default)]
    pub flux: Option<FluxConfig>,

    #[serde(default)]
    pub scenarios: Vec<Scenario>,

    /// EKS node group management for burst testing
    #[serde(default)]
    pub node_group: Option<NodeGroupConfig>,

    /// `DaemonSet` warmup configuration for image pre-pull after node scale-up.
    #[serde(default)]
    pub warmup_daemonset: Option<WarmupConfig>,

    /// Namespace where injection gateway and webhook live.
    #[serde(default = "default_injection_namespace")]
    pub injection_namespace: String,

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

    /// Deployment name for the gateway (used by `kubectl scale`).
    #[serde(default)]
    pub gateway_deployment: String,

    /// Deployment name for the webhook (used by `kubectl scale`).
    #[serde(default)]
    pub webhook_deployment: String,

    /// Injection detection mode: "sidecar" (2+ containers) or "env" (env var prefix match).
    #[serde(default = "default_injection_mode")]
    pub injection_mode: InjectionMode,

    /// Environment variable prefix used for injection detection in "env" mode.
    #[serde(default = "default_injection_env_prefix")]
    pub injection_env_prefix: String,

    /// Confluence reporting — auto-publish results after matrix run.
    #[serde(default)]
    pub confluence: Option<ConfluenceConfig>,

    /// Whether to require all gates to pass (default: true).
    ///
    /// When true, any gate failure aborts the scenario immediately.
    /// When false, gate failures are logged as warnings but execution continues.
    #[serde(default = "default_true")]
    pub strict_gates: bool,

    /// Backward-compatible fields — migrated to structured configs above.
    /// Prefer `image_cache.registry` over this field.
    #[serde(default)]
    pub cache_registry: Option<String>,

    /// Backward-compatible: prefer `flux.kustomizations`.
    #[serde(default)]
    pub flux_kustomizations: Vec<String>,
}

impl Config {
    /// Resolved pod label selector. Falls back to `app={deployment}` if not set.
    #[must_use]
    pub fn resolved_pod_label(&self) -> String {
        self.pod_label
            .clone()
            .unwrap_or_else(|| format!("app={}", self.deployment))
    }

    /// Resolved image cache namespace.
    #[must_use]
    pub fn image_cache_namespace(&self) -> String {
        self.image_cache
            .as_ref()
            .map_or_else(|| "image-cache".to_string(), |ic| ic.namespace.clone())
    }

    /// Resolved image cache label selector.
    #[must_use]
    pub fn image_cache_label(&self) -> String {
        self.image_cache
            .as_ref()
            .map_or_else(default_image_cache_label, |ic| ic.label.clone())
    }

    /// Resolved cache registry URL.
    #[must_use]
    pub fn resolved_cache_registry(&self) -> Option<String> {
        self.image_cache
            .as_ref()
            .map(|ic| ic.registry.clone())
            .or_else(|| self.cache_registry.clone())
    }

    /// Resolved flux namespace.
    #[must_use]
    pub fn flux_namespace(&self) -> String {
        self.flux
            .as_ref()
            .map_or_else(default_flux_namespace, |f| f.namespace.clone())
    }

    /// Resolved flux kustomizations list (prefers structured config).
    #[must_use]
    pub fn resolved_flux_kustomizations(&self) -> &[String] {
        if let Some(f) = &self.flux
            && !f.kustomizations.is_empty()
        {
            return &f.kustomizations;
        }
        &self.flux_kustomizations
    }
}

/// Confluence reporting configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfluenceConfig {
    /// Confluence base URL (e.g. "myorg.atlassian.net")
    pub base_url: String,
    /// Space key for reports
    pub space_key: String,
    /// Parent page ID — reports created as children
    pub parent_page_id: String,
    /// User email for Basic auth
    pub user_email: String,
    /// Path to API token file (default: ~/.config/atlassian/api-token)
    /// Also checks `CONFLUENCE_API_TOKEN` env var first.
    #[serde(default = "default_token_path")]
    pub token_path: String,
}

fn default_token_path() -> String {
    dirs::config_dir().map_or_else(
        || "~/.config/atlassian/api-token".to_string(),
        |d| d.join("atlassian").join("api-token").to_string_lossy().to_string(),
    )
}

/// How burst-forge detects successful secret injection.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum InjectionMode {
    /// Sidecar injection: 2+ containers indicates a sidecar was injected.
    Sidecar,
    /// Environment-variable injection: env vars matching `injection_env_prefix` present on any container.
    #[default]
    Env,
}

fn default_true() -> bool { true }
fn default_namespace() -> String { "scale-test".to_string() }
fn default_deployment() -> String { "nginx-burst".to_string() }
fn default_timeout() -> u64 { 600 }
fn default_poll_interval() -> u64 { 5 }
fn default_cooldown() -> u64 { 15 }
fn default_rollout_wait() -> u64 { 120 }
fn default_drain_timeout() -> u64 { 120 }
fn default_drain_poll_interval() -> u64 { 5 }
fn default_node_poll_interval() -> u64 { 15 }
fn default_replicas() -> u32 { 50 }
fn default_one() -> u32 { 1 }
fn default_region() -> String { "us-east-1".to_string() }
fn default_pods_per_node() -> u32 { 58 }
fn default_max_nodes() -> u32 { 20 }
fn default_injection_namespace() -> String { "injection-system".to_string() }
fn default_gateway_label() -> String { String::new() }
fn default_webhook_label() -> String { String::new() }
fn default_gateway_release() -> String { String::new() }
fn default_webhook_release() -> String { String::new() }
fn default_injection_env_prefix() -> String { "AKEYLESS_".to_string() }
fn default_injection_mode() -> InjectionMode { InjectionMode::Env }
fn default_image_cache_namespace() -> String { "image-cache".to_string() }
fn default_image_cache_label() -> String { "app.kubernetes.io/name=zot".to_string() }
fn default_flux_namespace() -> String { "flux-system".to_string() }
fn default_warmup_timeout() -> u64 { 300 }

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
