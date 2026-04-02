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

    /// Inject a sleep into the customer-init container to model realistic init latency.
    /// Value in seconds. 0 or absent = no sleep (uses deployment template as-is).
    #[serde(default)]
    pub init_sleep_secs: Option<u32>,

    /// Override pod memory request (e.g., "30Gi") for scheduling pressure testing.
    #[serde(default)]
    pub pod_memory_request: Option<String>,

    /// Override pod memory limit. If absent, mirrors request.
    #[serde(default)]
    pub pod_memory_limit: Option<String>,

    /// Expected number of secrets per pod. Used for injection count verification.
    /// When set, burst-forge counts individual injected secrets (not just presence).
    #[serde(default)]
    pub expected_secrets: Option<u32>,

    /// Override webhook CPU request per scenario (e.g., "50m").
    #[serde(default)]
    pub webhook_cpu_request: Option<String>,
    /// Override webhook CPU limit per scenario (e.g., "100m"). Use "0" to remove limit.
    #[serde(default)]
    pub webhook_cpu_limit: Option<String>,
    /// Override gateway CPU request per scenario (e.g., "200m").
    #[serde(default)]
    pub gateway_cpu_request: Option<String>,
    /// Override gateway CPU limit per scenario (e.g., "1000m").
    #[serde(default)]
    pub gateway_cpu_limit: Option<String>,
    /// Override gateway memory request per scenario (e.g., "256Mi").
    #[serde(default)]
    pub gateway_memory_request: Option<String>,
    /// Override gateway memory limit per scenario (e.g., "1Gi").
    #[serde(default)]
    pub gateway_memory_limit: Option<String>,
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

/// Worker node group configuration (optional — for managing infra node scaling).
///
/// When configured, burst-forge scales the worker node group to `desired` before
/// the matrix and back to `baseline` after cleanup. If absent, worker scaling
/// is not managed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerNodeGroupConfig {
    /// EKS cluster name (reuses from `node_group` if same cluster).
    pub cluster_name: String,
    /// Worker node group name (e.g., "scale-test-workers").
    pub nodegroup_name: String,
    /// AWS region.
    #[serde(default = "default_region")]
    pub region: String,
    /// AWS profile.
    #[serde(default)]
    pub aws_profile: Option<String>,
    /// Desired worker count during experiments.
    #[serde(default = "default_worker_desired")]
    pub desired: u32,
    /// Baseline worker count to return to after experiments.
    #[serde(default = "default_worker_baseline")]
    pub baseline: u32,
    /// Max worker node group size (used in AWS scaling config).
    #[serde(default = "default_worker_max_nodes")]
    pub max_nodes: u32,
}

fn default_worker_desired() -> u32 { 3 }
fn default_worker_baseline() -> u32 { 3 }
fn default_worker_max_nodes() -> u32 { 6 }

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

    /// Workload kind: "deployment" (default) or "job".
    #[serde(default)]
    pub workload_kind: WorkloadKind,

    /// Path to Job template YAML (required when `workload_kind: job`).
    #[serde(default)]
    pub job_template: Option<String>,

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

    /// Kubeconfig path override. When set, burst-forge uses this instead of
    /// the `--kubeconfig` CLI flag or `KUBECONFIG` env var.
    /// This keeps the kubeconfig in the declarative config alongside everything else.
    #[serde(default)]
    pub kubeconfig: Option<String>,

    /// Seconds to wait after image warmup for IPAMD to attach secondary ENIs.
    /// Custom networking requires 2-3 minutes for ENI setup on new nodes.
    /// Set to 0 to skip (default). Recommended: 150 for custom networking.
    #[serde(default)]
    pub ipamd_warmup_secs: u64,

    /// FluxCD kustomization names to suspend during burst and resume after cleanup.
    /// Prevents GitOps from reverting deployment replica counts mid-experiment.
    #[serde(default)]
    pub suspend_kustomizations: Vec<String>,

    /// Namespace where `suspend_kustomizations` live.
    #[serde(default = "default_flux_namespace")]
    pub suspend_kustomizations_namespace: String,

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

    /// EKS node group management for burst testing (burst pods).
    #[serde(default)]
    pub node_group: Option<NodeGroupConfig>,

    /// Worker node group management (infrastructure pods: gateway + webhook).
    /// When configured, burst-forge scales workers to `desired` before experiments
    /// and back to `baseline` after cleanup. Ensures infrastructure has the right
    /// capacity before any scenario fires.
    #[serde(default)]
    pub worker_node_group: Option<WorkerNodeGroupConfig>,

    /// Observability node group — scales to 1 during warmup, 0 on cleanup.
    /// Hosts VictoriaMetrics, Grafana, Loki for live experiment metrics.
    #[serde(default)]
    pub observability_node_group: Option<NodeGroupConfig>,

    /// Whether to verify teardown completed (burst nodes at 0, pods drained)
    /// before exiting. Default: true.
    #[serde(default = "default_true")]
    pub verify_teardown: bool,

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

    /// Container name for the init container in scenario patches.
    #[serde(default = "default_init_container_name")]
    pub init_container_name: String,

    /// Container name for the main workload container in scenario patches.
    #[serde(default = "default_workload_container_name")]
    pub workload_container_name: String,

    /// Container name inside the webhook deployment (for CPU patches).
    #[serde(default = "default_webhook_container_name")]
    pub webhook_container_name: String,

    /// Container name inside the gateway deployment (for CPU patches).
    #[serde(default = "default_gateway_container_name")]
    pub gateway_container_name: String,

    /// Secret path prefix for multi-secret injection patches.
    /// First secret uses this path exactly; additional secrets append the index.
    #[serde(default = "default_secret_path_prefix")]
    pub secret_path_prefix: String,

    /// Confluence reporting — auto-publish results after matrix run.
    #[serde(default)]
    pub confluence: Option<ConfluenceConfig>,

    /// Directory for JSON/CSV result export. When set, writes
    /// `results-{timestamp}.json` after each matrix run.
    #[serde(default)]
    pub output_dir: Option<String>,

    /// Reset phase configuration (Phase 1).
    #[serde(default)]
    pub reset: ResetConfig,

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

/// Reset phase (Phase 1) configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResetConfig {
    /// Force delete immediately instead of waiting for graceful termination.
    #[serde(default)]
    pub force_delete: bool,
    /// Seconds to wait for graceful termination before force deleting
    /// (only used when `force_delete` is false).
    #[serde(default = "default_grace_period")]
    pub grace_period_secs: u64,
}

impl Default for ResetConfig {
    fn default() -> Self {
        Self {
            force_delete: false,
            grace_period_secs: default_grace_period(),
        }
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

/// Workload kind for burst tests.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WorkloadKind {
    /// Scale a Deployment (kubectl scale deployment --replicas=N).
    #[default]
    Deployment,
    /// Create N individual Jobs from a template.
    Job,
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
fn default_init_container_name() -> String { "customer-init".to_string() }
fn default_workload_container_name() -> String { "nginx".to_string() }
fn default_webhook_container_name() -> String { "akeyless-secrets-injection".to_string() }
fn default_gateway_container_name() -> String { "api-gateway".to_string() }
fn default_secret_path_prefix() -> String { "/pleme/test/hello".to_string() }
fn default_injection_mode() -> InjectionMode { InjectionMode::Env }
fn default_image_cache_namespace() -> String { "image-cache".to_string() }
fn default_image_cache_label() -> String { "app.kubernetes.io/name=zot".to_string() }
fn default_flux_namespace() -> String { "flux-system".to_string() }
fn default_warmup_timeout() -> u64 { 300 }
fn default_grace_period() -> u64 { 30 }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_from_empty_json() {
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.namespace, "scale-test");
        assert_eq!(cfg.deployment, "nginx-burst");
        assert_eq!(cfg.timeout_secs, 600);
        assert_eq!(cfg.poll_interval_secs, 5);
        assert_eq!(cfg.cooldown_secs, 15);
        assert_eq!(cfg.drain_timeout_secs, 120);
        assert_eq!(cfg.init_container_name, "customer-init");
        assert_eq!(cfg.workload_container_name, "nginx");
        assert_eq!(cfg.webhook_container_name, "akeyless-secrets-injection");
        assert_eq!(cfg.gateway_container_name, "api-gateway");
        assert_eq!(cfg.secret_path_prefix, "/pleme/test/hello");
    }

    #[test]
    fn resolved_pod_label_default() {
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.resolved_pod_label(), "app=nginx-burst");
    }

    #[test]
    fn resolved_pod_label_override() {
        let cfg: Config =
            serde_json::from_str(r#"{"pod_label": "custom=label"}"#).unwrap();
        assert_eq!(cfg.resolved_pod_label(), "custom=label");
    }

    #[test]
    fn scenario_defaults() {
        let s: Scenario = serde_json::from_str(r#"{"name":"test"}"#).unwrap();
        assert_eq!(s.replicas, 50);
        assert_eq!(s.gateway_replicas, 1);
        assert_eq!(s.webhook_replicas, 1);
        assert!(s.init_sleep_secs.is_none());
        assert!(s.expected_secrets.is_none());
    }

    #[test]
    fn all_configs_parse() {
        let configs_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("configs");
        if !configs_dir.exists() {
            return;
        }
        for entry in std::fs::read_dir(&configs_dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "yaml") {
                let result = discover(Some(&path.to_string_lossy()));
                assert!(
                    result.is_ok(),
                    "Failed to parse {}: {}",
                    path.display(),
                    result.unwrap_err()
                );
            }
        }
    }
}
