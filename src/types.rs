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

// --- Enhanced observability types (Shinryū integration) ---

/// Detailed per-pod state extracted from kubectl JSON.
/// Captures the fields that `count_pod_states()` previously ignored.
#[derive(Debug, Serialize, Deserialize)]
pub struct PodDetail {
    pub name: String,
    pub phase: String,
    pub node: Option<String>,
    pub creation_timestamp: Option<String>,
    pub restart_count: u32,
    pub state_reason: Option<String>,
    pub container_started_at: Option<String>,
    pub qos_class: Option<String>,
    pub host_ip: Option<String>,
    pub pod_ip: Option<String>,
    pub injected: bool,
}

impl PodDetail {
    /// Extract `PodDetail` from a kubectl pod JSON value.
    pub fn from_json(pod: &serde_json::Value, injected: bool) -> Self {
        let status = &pod["status"];
        let container_status = status["containerStatuses"]
            .as_array()
            .and_then(|a| a.first());

        Self {
            name: pod["metadata"]["name"].as_str().unwrap_or("").to_string(),
            phase: status["phase"].as_str().unwrap_or("Unknown").to_string(),
            node: pod["spec"]["nodeName"].as_str().map(String::from),
            creation_timestamp: pod["metadata"]["creationTimestamp"].as_str().map(String::from),
            restart_count: container_status
                .and_then(|c| c["restartCount"].as_u64())
                .unwrap_or(0) as u32,
            state_reason: container_status
                .and_then(|c| c["state"]["waiting"]["reason"].as_str())
                .map(String::from),
            container_started_at: container_status
                .and_then(|c| c["state"]["running"]["startedAt"].as_str())
                .map(String::from),
            qos_class: status["qosClass"].as_str().map(String::from),
            host_ip: status["hostIP"].as_str().map(String::from),
            pod_ip: status["podIP"].as_str().map(String::from),
            injected,
        }
    }
}

// --- Shared rate functions ---

/// Calculate the injection success rate as a percentage.
/// Returns 0.0 when running is 0 (no pods to measure against).
#[must_use]
pub fn injection_rate(running: u32, injected: u32) -> f64 {
    if running > 0 {
        f64::from(injected) / f64::from(running) * 100.0
    } else {
        0.0
    }
}

/// Calculate throughput (items per second) from a count and duration in milliseconds.
/// Returns 0.0 when `elapsed_ms` is 0.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn throughput_per_sec(count: u32, elapsed_ms: u64) -> f64 {
    if elapsed_ms > 0 {
        f64::from(count) / (elapsed_ms as f64 / 1000.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injection_rate_full() {
        let rate = injection_rate(100, 100);
        assert!((rate - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn injection_rate_partial() {
        let rate = injection_rate(100, 50);
        assert!((rate - 50.0).abs() < f64::EPSILON);
    }

    #[test]
    fn injection_rate_zero_running() {
        assert!((injection_rate(0, 50)).abs() < f64::EPSILON);
    }

    #[test]
    fn injection_rate_zero_injected() {
        assert!((injection_rate(100, 0)).abs() < f64::EPSILON);
    }

    #[test]
    fn throughput_1000ms() {
        let t = throughput_per_sec(100, 1000);
        assert!((t - 100.0).abs() < 0.01);
    }

    #[test]
    fn throughput_500ms() {
        let t = throughput_per_sec(100, 500);
        assert!((t - 200.0).abs() < 0.01);
    }

    #[test]
    fn throughput_zero_time() {
        assert!((throughput_per_sec(100, 0)).abs() < f64::EPSILON);
    }

    #[test]
    fn throughput_zero_count() {
        assert!((throughput_per_sec(0, 1000)).abs() < f64::EPSILON);
    }
}
