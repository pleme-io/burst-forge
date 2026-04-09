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
    /// Override webhook memory request per scenario (e.g., "128Mi").
    #[serde(default)]
    pub webhook_memory_request: Option<String>,
    /// Override webhook memory limit per scenario (e.g., "512Mi").
    #[serde(default)]
    pub webhook_memory_limit: Option<String>,

    /// Per-infrastructure-deployment replica counts (keyed by deployment name).
    /// Used when `infrastructure_deployments` is configured.
    /// Falls back to gateway_replicas/webhook_replicas for legacy configs.
    #[serde(default)]
    pub infra_replicas: std::collections::HashMap<String, u32>,
}

impl Scenario {
    /// Get the target replica count for a named infrastructure deployment.
    #[must_use]
    pub fn replicas_for(&self, deployment_name: &str) -> u32 {
        if let Some(&r) = self.infra_replicas.get(deployment_name) {
            return r;
        }
        match deployment_name {
            "gateway" => self.gateway_replicas,
            "webhook" => self.webhook_replicas,
            _ => 1,
        }
    }
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

/// Infrastructure node group configuration for the dedicated gateway and
/// webhook node pools.
///
/// Unlike `WorkerNodeGroupConfig` (which uses a fixed `desired` size for the
/// whole experiment), infra node groups scale dynamically per scenario:
/// `desired = ceil(deployment_replicas / pods_per_node).clamp(baseline, max_nodes)`.
///
/// This lets a single scenario like `gw16-wh12-1000-isolated` automatically
/// provision the right number of `m5.large` nodes for 16 GW pods at 1536Mi
/// each (4 pods/node × 4 nodes), without any manual `aws eks` scaling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InfraNodeGroupConfig {
    /// EKS cluster name.
    pub cluster_name: String,
    /// Node group name (e.g., "scale-test-gateway", "scale-test-webhook").
    pub nodegroup_name: String,
    /// AWS region.
    #[serde(default = "default_region")]
    pub region: String,
    /// AWS profile.
    #[serde(default)]
    pub aws_profile: Option<String>,
    /// How many GW or WH pods fit on one node of this group's instance type.
    /// E.g., for m5.large (8 GiB) with 1536Mi memory request, 4 pods fit.
    #[serde(default = "default_infra_pods_per_node")]
    pub pods_per_node: u32,
    /// Baseline node count to return to between/after scenarios.
    /// Default 1 keeps a single warm node for the always-on baseline GW/WH pod.
    #[serde(default = "default_infra_baseline")]
    pub baseline: u32,
    /// Hard ceiling on the node group size.
    #[serde(default = "default_infra_max_nodes")]
    pub max_nodes: u32,
    /// Pad the computed desired count by this many extra nodes for headroom.
    /// Useful when pods_per_node is a tight ceiling and you want bin-packing
    /// slack so the scheduler doesn't fail under transient eviction.
    #[serde(default)]
    pub headroom_nodes: u32,
}

fn default_infra_pods_per_node() -> u32 { 4 }
fn default_infra_baseline() -> u32 { 1 }
fn default_infra_max_nodes() -> u32 { 16 }

impl InfraNodeGroupConfig {
    /// Compute the desired node count for the given pod count, clamped between
    /// `baseline` and `max_nodes`.
    #[must_use]
    pub fn desired_for_pods(&self, pod_count: u32) -> u32 {
        let raw = pod_count.div_ceil(self.pods_per_node.max(1));
        let with_headroom = raw.saturating_add(self.headroom_nodes);
        with_headroom.clamp(self.baseline, self.max_nodes)
    }
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

    /// Dedicated gateway node group. When configured, burst-forge scales it
    /// dynamically per scenario based on `gateway_replicas / pods_per_node`
    /// before patching the GW deployment, and back to baseline at cleanup.
    /// Required for any scenario where the GW deployment exceeds what fits on
    /// the baseline nodegroup size.
    #[serde(default)]
    pub gateway_node_group: Option<InfraNodeGroupConfig>,

    /// Dedicated webhook node group. Same dynamic scaling pattern as
    /// `gateway_node_group`, sized from `webhook_replicas`.
    #[serde(default)]
    pub webhook_node_group: Option<InfraNodeGroupConfig>,

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

    /// JSON merge-patch template for gateway HelmRelease replica count.
    /// Use `{replicas}` as placeholder. Default for akeyless-gateway chart:
    /// `{"spec":{"values":{"gateway":{"deployment":{"replicaCount":{replicas}}}}}}`
    /// Old akeyless-api-gateway chart used:
    /// `{"spec":{"values":{"replicaCount":{replicas}}}}`
    #[serde(default = "default_gateway_replica_patch")]
    pub gateway_replica_patch: String,

    /// JSON merge-patch template for webhook HelmRelease replica count.
    #[serde(default = "default_webhook_replica_patch")]
    pub webhook_replica_patch: String,

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

    /// Gateway QPS limit (per replica). Used by scaling formulas for predictions.
    /// Default: 5 (Akeyless default).
    #[serde(default = "default_qps")]
    pub qps: u32,

    /// Maximum number of gateway replicas to add per scaling wave.
    /// When > 0, Phase 2c scales the gateway in waves of this size,
    /// waiting for each wave to become Ready before adding the next.
    /// This avoids rate-limit saturation against the Akeyless SaaS API
    /// during cold-start — each pod does a full-sync, and the SaaS
    /// throttles at ~5 concurrent auth requests per access-id (ASM-17539).
    /// Default: 0 (no batching — scale all at once, legacy behavior).
    #[serde(default)]
    pub gateway_batch_size: u32,

    /// Maximum number of workload pods to create per burst wave.
    /// When > 0, Phase 3 scales the deployment in waves of this size,
    /// waiting briefly between waves for the webhook to process admission.
    /// This avoids webhook timeout at 10000+ pods — the webhook can only
    /// handle ~160-320 pod creates/sec, so 10000 simultaneous creates
    /// exceed the 30s timeout. Waves of 2500 stay within limits.
    /// Default: 0 (no batching — all pods at once, legacy behavior).
    #[serde(default)]
    pub burst_batch_size: u32,

    /// Seconds to wait between burst batches for webhook to drain.
    /// Default: 5.
    #[serde(default = "default_burst_batch_wait")]
    pub burst_batch_wait_secs: u64,

    /// Secrets per pod for injection counting and prediction calculation.
    /// Default: 2.
    #[serde(default = "default_secrets_per_pod")]
    pub secrets_per_pod: u32,

    /// Confluence reporting — auto-publish results after matrix run.
    #[serde(default)]
    pub confluence: Option<ConfluenceConfig>,

    /// Directory for JSON/CSV result export. When set, writes
    /// `results-{timestamp}.json` after each matrix run.
    #[serde(default)]
    pub output_dir: Option<String>,

    /// Vector HTTP endpoint for structured event emission (Shinryū integration).
    /// When set, burst-forge POSTs JSON events to this URL.
    /// Example: `http://vector.observability.svc:9500`
    #[serde(default)]
    pub vector_endpoint: Option<String>,

    /// Reset phase configuration (Phase 1).
    #[serde(default)]
    pub reset: ResetConfig,

    /// Whether to require all gates to pass (default: true).
    ///
    /// When true, any gate failure aborts the scenario immediately.
    /// When false, gate failures are logged as warnings but execution continues.
    #[serde(default = "default_true")]
    pub strict_gates: bool,

    /// Cluster-autoscaler management. When present, burst-forge pauses the
    /// autoscaler at the start of each flow and resumes in cleanup. This
    /// prevents node drain during experiments.
    #[serde(default = "default_autoscaler")]
    pub autoscaler: Option<AutoscalerConfig>,

    /// Gateway readiness threshold for gates (0.0-1.0).
    /// Default 0.9 — 90% of GW pods must be Ready to pass.
    /// Set to 1.0 for strict mode.
    #[serde(default = "default_gw_readiness_threshold")]
    pub gate_gw_readiness_threshold: f64,

    /// Annotation key for secret injection on burst pods.
    /// Used when patching pod annotations for multi-secret scenarios.
    #[serde(default = "default_injection_annotation_key")]
    pub injection_annotation_key: String,

    /// Generic infrastructure deployments. When present, replaces the legacy
    /// gateway_*/webhook_* fields. Each deployment has its own scaling order,
    /// readiness threshold, and strategy.
    #[serde(default)]
    pub infrastructure_deployments: Vec<InfraDeployment>,

    /// Backward-compatible fields — migrated to structured configs above.
    /// Prefer `image_cache.registry` over this field.
    #[serde(default)]
    pub cache_registry: Option<String>,

    /// Backward-compatible: prefer `flux.kustomizations`.
    #[serde(default)]
    pub flux_kustomizations: Vec<String>,
}

/// Cluster-autoscaler configuration for pause/resume during experiments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoscalerConfig {
    /// Deployment name (e.g., "cluster-autoscaler-aws-cluster-autoscaler").
    #[serde(default = "default_autoscaler_deployment")]
    pub deployment_name: String,

    /// Namespace (e.g., "kube-system").
    #[serde(default = "default_autoscaler_namespace")]
    pub namespace: String,

    /// Replica count to restore on resume (default: 1).
    #[serde(default = "default_autoscaler_replicas")]
    pub replicas: u32,
}

/// Scaling strategy for infrastructure deployments.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScalingStrategy {
    /// Suspend HelmRelease, kubectl scale deployment, resume HR in cleanup.
    #[default]
    SuspendAndScale,
    /// Patch HelmRelease values directly.
    HelmreleasePatch,
    /// Just kubectl scale — for non-Flux-managed deployments.
    DirectScale,
}

/// Generic infrastructure deployment (replaces separate gateway/webhook concepts).
/// Each deployment has its own readiness threshold, scaling order, and strategy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InfraDeployment {
    /// Human-readable name (e.g., "gateway", "webhook").
    pub name: String,
    /// Kubernetes namespace.
    pub namespace: String,
    /// Deployment name (for kubectl scale).
    pub deployment: String,
    /// HelmRelease name (for suspend/resume). Empty = no HelmRelease.
    #[serde(default)]
    pub helmrelease: String,
    /// Label selector for pod queries.
    #[serde(default)]
    pub label: String,
    /// Container name inside the deployment (for resource patches).
    #[serde(default)]
    pub container_name: String,
    /// Readiness threshold (0.0-1.0). 0.9 = 90% pods Ready to pass gate.
    #[serde(default = "default_full_threshold")]
    pub readiness_threshold: f64,
    /// Scaling order (lower = scales first). Same order = sequential in list order.
    #[serde(default)]
    pub scaling_order: u32,
    /// How to scale this deployment.
    #[serde(default)]
    pub scaling_strategy: ScalingStrategy,
    /// Wave batch size for gradual scaling (0 = all at once).
    #[serde(default)]
    pub batch_size: u32,
    /// Dedicated node group (if applicable).
    #[serde(default)]
    pub node_group: Option<InfraNodeGroupConfig>,
    /// HelmRelease replica patch template (JSON with {replicas} placeholder).
    #[serde(default)]
    pub replica_patch: String,
}

fn default_full_threshold() -> f64 { 1.0 }

impl Config {
    /// Resolve infrastructure deployments from either the new
    /// `infrastructure_deployments` field or the legacy gateway/webhook fields.
    #[must_use]
    pub fn resolved_infra_deployments(&self) -> Vec<InfraDeployment> {
        if !self.infrastructure_deployments.is_empty() {
            return self.infrastructure_deployments.clone();
        }
        let mut deployments = Vec::new();
        if !self.gateway_deployment.is_empty() {
            deployments.push(InfraDeployment {
                name: "gateway".to_string(),
                namespace: self.injection_namespace.clone(),
                deployment: self.gateway_deployment.clone(),
                helmrelease: self.gateway_release.clone(),
                label: self.gateway_label.clone(),
                container_name: self.gateway_container_name.clone(),
                readiness_threshold: self.gate_gw_readiness_threshold,
                scaling_order: 0,
                scaling_strategy: ScalingStrategy::SuspendAndScale,
                batch_size: self.gateway_batch_size,
                node_group: self.gateway_node_group.clone(),
                replica_patch: self.gateway_replica_patch.clone(),
            });
        }
        if !self.webhook_deployment.is_empty() {
            deployments.push(InfraDeployment {
                name: "webhook".to_string(),
                namespace: self.injection_namespace.clone(),
                deployment: self.webhook_deployment.clone(),
                helmrelease: self.webhook_release.clone(),
                label: self.webhook_label.clone(),
                container_name: self.webhook_container_name.clone(),
                readiness_threshold: 1.0,
                scaling_order: 1,
                scaling_strategy: ScalingStrategy::SuspendAndScale,
                batch_size: 0,
                node_group: self.webhook_node_group.clone(),
                replica_patch: self.webhook_replica_patch.clone(),
            });
        }
        deployments
    }

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
fn default_gateway_replica_patch() -> String {
    r#"{"spec":{"values":{"gateway":{"deployment":{"replicaCount":{replicas}}}}}}"#.to_string()
}
fn default_webhook_replica_patch() -> String {
    r#"{"spec":{"values":{"replicaCount":{replicas}}}}"#.to_string()
}
fn default_injection_env_prefix() -> String { String::new() }
fn default_init_container_name() -> String { String::new() }
fn default_workload_container_name() -> String { String::new() }
fn default_webhook_container_name() -> String { String::new() }
fn default_gateway_container_name() -> String { String::new() }
fn default_secret_path_prefix() -> String { String::new() }
fn default_injection_mode() -> InjectionMode { InjectionMode::Env }
fn default_image_cache_namespace() -> String { "image-cache".to_string() }
fn default_image_cache_label() -> String { "app.kubernetes.io/name=zot".to_string() }
fn default_flux_namespace() -> String { "flux-system".to_string() }
fn default_qps() -> u32 { 5 }
fn default_secrets_per_pod() -> u32 { 2 }
fn default_burst_batch_wait() -> u64 { 5 }
fn default_warmup_timeout() -> u64 { 300 }
fn default_grace_period() -> u64 { 30 }
fn default_autoscaler() -> Option<AutoscalerConfig> { None }
fn default_autoscaler_deployment() -> String { String::new() }
fn default_autoscaler_namespace() -> String { "kube-system".to_string() }
fn default_autoscaler_replicas() -> u32 { 1 }
fn default_gw_readiness_threshold() -> f64 { 0.9 }
fn default_injection_annotation_key() -> String { String::new() }

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
        assert!(cfg.init_container_name.is_empty());
        assert!(cfg.workload_container_name.is_empty());
        assert!(cfg.webhook_container_name.is_empty());
        assert!(cfg.gateway_container_name.is_empty());
        assert!(cfg.secret_path_prefix.is_empty());
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

    #[test]
    fn image_cache_namespace_default() {
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.image_cache_namespace(), "image-cache");
    }

    #[test]
    fn image_cache_namespace_override() {
        let cfg: Config = serde_json::from_str(
            r#"{"image_cache": {"namespace": "custom-ns", "label": "app=zot", "registry": "zot:5000"}}"#,
        ).unwrap();
        assert_eq!(cfg.image_cache_namespace(), "custom-ns");
    }

    #[test]
    fn image_cache_label_default() {
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.image_cache_label(), "app.kubernetes.io/name=zot");
    }

    #[test]
    fn image_cache_label_override() {
        let cfg: Config = serde_json::from_str(
            r#"{"image_cache": {"namespace": "ns", "label": "app=custom-zot", "registry": "r:5000"}}"#,
        ).unwrap();
        assert_eq!(cfg.image_cache_label(), "app=custom-zot");
    }

    #[test]
    fn resolved_cache_registry_none_by_default() {
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert!(cfg.resolved_cache_registry().is_none());
    }

    #[test]
    fn resolved_cache_registry_from_image_cache() {
        let cfg: Config = serde_json::from_str(
            r#"{"image_cache": {"namespace": "ns", "label": "l", "registry": "zot.local:5000"}}"#,
        ).unwrap();
        assert_eq!(cfg.resolved_cache_registry(), Some("zot.local:5000".to_string()));
    }

    #[test]
    fn resolved_cache_registry_legacy_fallback() {
        let cfg: Config = serde_json::from_str(
            r#"{"cache_registry": "legacy:5000"}"#,
        ).unwrap();
        assert_eq!(cfg.resolved_cache_registry(), Some("legacy:5000".to_string()));
    }

    #[test]
    fn resolved_cache_registry_prefers_structured() {
        let cfg: Config = serde_json::from_str(
            r#"{"image_cache": {"namespace": "ns", "label": "l", "registry": "new:5000"}, "cache_registry": "old:5000"}"#,
        ).unwrap();
        assert_eq!(cfg.resolved_cache_registry(), Some("new:5000".to_string()));
    }

    #[test]
    fn flux_namespace_default() {
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.flux_namespace(), "flux-system");
    }

    #[test]
    fn flux_namespace_override() {
        let cfg: Config = serde_json::from_str(
            r#"{"flux": {"namespace": "custom-flux", "kustomizations": []}}"#,
        ).unwrap();
        assert_eq!(cfg.flux_namespace(), "custom-flux");
    }

    #[test]
    fn resolved_flux_kustomizations_empty_by_default() {
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert!(cfg.resolved_flux_kustomizations().is_empty());
    }

    #[test]
    fn resolved_flux_kustomizations_from_structured() {
        let cfg: Config = serde_json::from_str(
            r#"{"flux": {"namespace": "ns", "kustomizations": ["a", "b"]}}"#,
        ).unwrap();
        assert_eq!(cfg.resolved_flux_kustomizations(), &["a", "b"]);
    }

    #[test]
    fn resolved_flux_kustomizations_legacy_fallback() {
        let cfg: Config = serde_json::from_str(
            r#"{"flux_kustomizations": ["legacy-a"]}"#,
        ).unwrap();
        assert_eq!(cfg.resolved_flux_kustomizations(), &["legacy-a"]);
    }

    #[test]
    fn resolved_flux_kustomizations_prefers_structured() {
        let cfg: Config = serde_json::from_str(
            r#"{"flux": {"namespace": "ns", "kustomizations": ["new"]}, "flux_kustomizations": ["old"]}"#,
        ).unwrap();
        assert_eq!(cfg.resolved_flux_kustomizations(), &["new"]);
    }

    #[test]
    fn resolved_flux_kustomizations_empty_structured_falls_back() {
        let cfg: Config = serde_json::from_str(
            r#"{"flux": {"namespace": "ns", "kustomizations": []}, "flux_kustomizations": ["fallback"]}"#,
        ).unwrap();
        assert_eq!(cfg.resolved_flux_kustomizations(), &["fallback"]);
    }

    #[test]
    fn injection_mode_serde() {
        let cfg: Config = serde_json::from_str(r#"{"injection_mode": "sidecar"}"#).unwrap();
        assert_eq!(cfg.injection_mode, InjectionMode::Sidecar);

        let cfg: Config = serde_json::from_str(r#"{"injection_mode": "env"}"#).unwrap();
        assert_eq!(cfg.injection_mode, InjectionMode::Env);
    }

    #[test]
    fn injection_mode_default_is_env() {
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.injection_mode, InjectionMode::Env);
    }

    #[test]
    fn workload_kind_serde() {
        let cfg: Config = serde_json::from_str(r#"{"workload_kind": "job"}"#).unwrap();
        assert_eq!(cfg.workload_kind, WorkloadKind::Job);

        let cfg: Config = serde_json::from_str(r#"{"workload_kind": "deployment"}"#).unwrap();
        assert_eq!(cfg.workload_kind, WorkloadKind::Deployment);
    }

    #[test]
    fn workload_kind_default_is_deployment() {
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.workload_kind, WorkloadKind::Deployment);
    }

    #[test]
    fn reset_config_default() {
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert!(!cfg.reset.force_delete);
        assert_eq!(cfg.reset.grace_period_secs, 30);
    }

    #[test]
    fn reset_config_override() {
        let cfg: Config = serde_json::from_str(
            r#"{"reset": {"force_delete": true, "grace_period_secs": 5}}"#,
        ).unwrap();
        assert!(cfg.reset.force_delete);
        assert_eq!(cfg.reset.grace_period_secs, 5);
    }

    #[test]
    fn scenario_with_all_overrides() {
        let s: Scenario = serde_json::from_str(r#"{
            "name": "full",
            "replicas": 500,
            "gateway_replicas": 10,
            "webhook_replicas": 5,
            "nodes": 20,
            "init_sleep_secs": 3,
            "pod_memory_request": "4Gi",
            "pod_memory_limit": "8Gi",
            "expected_secrets": 4,
            "webhook_cpu_request": "100m",
            "webhook_cpu_limit": "200m",
            "gateway_cpu_request": "500m",
            "gateway_cpu_limit": "1000m",
            "gateway_memory_request": "512Mi",
            "gateway_memory_limit": "1Gi",
            "webhook_memory_request": "256Mi",
            "webhook_memory_limit": "512Mi"
        }"#).unwrap();
        assert_eq!(s.replicas, 500);
        assert_eq!(s.gateway_replicas, 10);
        assert_eq!(s.webhook_replicas, 5);
        assert_eq!(s.nodes, Some(20));
        assert_eq!(s.init_sleep_secs, Some(3));
        assert_eq!(s.pod_memory_request.as_deref(), Some("4Gi"));
        assert_eq!(s.pod_memory_limit.as_deref(), Some("8Gi"));
        assert_eq!(s.expected_secrets, Some(4));
        assert_eq!(s.webhook_cpu_request.as_deref(), Some("100m"));
        assert_eq!(s.gateway_memory_limit.as_deref(), Some("1Gi"));
    }

    #[test]
    fn config_verify_teardown_default_true() {
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert!(cfg.verify_teardown);
    }

    #[test]
    fn config_strict_gates_default_true() {
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert!(cfg.strict_gates);
    }

    #[test]
    fn config_default_injection_env_prefix() {
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert!(cfg.injection_env_prefix.is_empty());
    }

    #[test]
    fn config_default_secret_path_prefix() {
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert!(cfg.secret_path_prefix.is_empty());
    }

    #[test]
    fn config_default_qps_and_secrets() {
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.qps, 5);
        assert_eq!(cfg.secrets_per_pod, 2);
    }

    #[test]
    fn node_group_config_defaults() {
        let ng: NodeGroupConfig = serde_json::from_str(r#"{
            "cluster_name": "test-cluster",
            "nodegroup_name": "burst-ng"
        }"#).unwrap();
        assert_eq!(ng.region, "us-east-1");
        assert_eq!(ng.pods_per_node, 58);
        assert_eq!(ng.max_nodes, 20);
        assert!(ng.aws_profile.is_none());
    }

    #[test]
    fn worker_node_group_config_defaults() {
        let wng: WorkerNodeGroupConfig = serde_json::from_str(r#"{
            "cluster_name": "test-cluster",
            "nodegroup_name": "worker-ng"
        }"#).unwrap();
        assert_eq!(wng.desired, 3);
        assert_eq!(wng.baseline, 3);
        assert_eq!(wng.max_nodes, 6);
    }
}
