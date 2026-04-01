//! Shared types for burst-forge.

use serde::{Deserialize, Serialize};

/// Result of a single burst test iteration.
#[derive(Debug, Serialize, Deserialize)]
pub struct BurstResult {
    pub timestamp: String,
    pub replicas_requested: u32,
    pub pods_running: u32,
    pub pods_failed: u32,
    pub pods_pending: u32,
    pub pods_injected: u32,
    pub injection_success_rate: f64,
    pub time_to_first_ready_ms: u64,
    pub time_to_all_ready_ms: Option<u64>,
    /// Time until all pods were admitted (injected == replicas).
    pub time_to_full_admission_ms: Option<u64>,
    /// Time until 50% of pods were Running.
    pub time_to_50pct_running_ms: Option<u64>,
    /// Admission rate: pods/sec through webhook.
    pub admission_rate_pods_per_sec: f64,
    /// Gateway throughput: pods/sec through init container.
    pub gateway_throughput_pods_per_sec: f64,
    pub duration_ms: u64,
    pub nodes: u32,
    pub iteration: u32,
    /// Total individual secrets injected across all pods.
    #[serde(default)]
    pub total_secrets_injected: u32,
    /// Peak concurrent Running pods (useful for Jobs that complete and get cleaned up).
    #[serde(default)]
    pub peak_running: u32,
}

/// Result of infrastructure verification.
#[derive(Debug, Serialize, Deserialize)]
pub struct VerifyResult {
    pub node_count: usize,
    pub ready_nodes: usize,
    pub gateway_replicas: usize,
    pub webhook_replicas: usize,
    pub deployment_found: bool,
    pub image_cache: Option<ImageCacheCheck>,
}

/// Result of checking the image cache (Zot registry).
#[derive(Debug, Serialize, Deserialize)]
pub struct ImageCacheCheck {
    pub registry: String,
    pub images: Vec<ImageStatus>,
}

/// Status of a single image in the cache.
#[derive(Debug, Serialize, Deserialize)]
pub struct ImageStatus {
    pub image: String,
    pub cached: bool,
    pub tags: Vec<String>,
}

/// Warmup sub-phase timings (Phase 2).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WarmupTimings {
    /// Time for nodes to reach Ready+Schedulable (ms).
    pub nodes_ms: u64,
    /// Time for image warmup DaemonSet rollout (ms).
    pub images_ms: u64,
    /// Time for IPAMD secondary ENI warmup (ms).
    #[serde(default)]
    pub ipamd_warmup_ms: u64,
    /// Time for gateway deployment to reach desired replicas (ms).
    pub gateway_ms: u64,
    /// Time for webhook deployment to reach desired replicas (ms).
    pub webhook_ms: u64,
    /// Time for all gates to pass (ms).
    pub gates_ms: u64,
    /// Time for per-scenario pod spec patches (ms).
    #[serde(default)]
    pub patches_ms: u64,
    /// Total warmup time (ms).
    pub total_ms: u64,
}

/// Phase timing breakdown for a scenario.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PhaseTimings {
    /// Phase 1: RESET -- time to reach verified zero state (ms).
    pub reset_ms: u64,
    /// Phase 2: WARMUP -- total time to infrastructure ready (ms).
    pub warmup_ms: u64,
    /// Phase 2 sub-phase breakdown.
    pub warmup_detail: WarmupTimings,
    /// Phase 3: EXECUTION -- time for burst test (ms).
    pub execution_ms: u64,
}

/// Result of a single scaling scenario.
#[derive(Debug, Serialize, Deserialize)]
pub struct ScenarioResult {
    pub name: String,
    pub replicas: u32,
    pub gateway_replicas: u32,
    pub webhook_replicas: u32,
    pub verify: Option<VerifyResult>,
    pub burst: Option<BurstResult>,
    pub phase_timings: Option<PhaseTimings>,
    pub error: Option<String>,
}

/// Aggregated results from the scaling matrix.
#[derive(Debug, Serialize, Deserialize)]
pub struct MatrixReport {
    pub timestamp: String,
    pub scenarios: Vec<ScenarioResult>,
}
