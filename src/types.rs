//! Shared types for burst-forge.

use serde::{Deserialize, Serialize};

/// Predicted performance values from scaling formulas.
///
/// Calculated before each burst using validated formulas from 40+ experiments.
/// Emitted alongside actuals in BURST_COMPLETE events for Shinryu comparison.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prediction {
    /// Predicted optimal GW replicas (sub-90s formula).
    pub predicted_gw_replicas: u32,
    /// Predicted optimal WH replicas.
    pub predicted_wh_replicas: u32,
    /// Theoretical minimum injection time in seconds.
    pub predicted_min_secs: f64,
    /// Predicted throughput (pods/sec) at theoretical minimum.
    pub predicted_throughput_pods_per_sec: f64,
    /// Which formula was used ("sub_90s" or "sub_3min").
    pub formula: String,
    /// Actual GW replicas used in this scenario.
    pub actual_gw_replicas: u32,
    /// Actual WH replicas used in this scenario.
    pub actual_wh_replicas: u32,
}

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
    /// Scaling formula predictions (when available).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prediction: Option<Prediction>,
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

impl Prediction {
    /// Calculate predictions for a scenario using validated scaling formulas.
    #[must_use]
    pub fn calculate(
        replicas: u32,
        secrets_per_pod: u32,
        qps: u32,
        actual_gw: u32,
        actual_wh: u32,
    ) -> Self {
        use crate::plan;

        let predicted_gw = plan::gw_for_sub_90s(replicas, secrets_per_pod, qps);
        let predicted_wh = plan::wh_optimal(replicas);
        let predicted_min = plan::theoretical_min_secs(replicas, secrets_per_pod, actual_gw, qps);
        let predicted_throughput = if predicted_min > 0.0 {
            f64::from(replicas) / predicted_min
        } else {
            0.0
        };

        Self {
            predicted_gw_replicas: predicted_gw,
            predicted_wh_replicas: predicted_wh,
            predicted_min_secs: predicted_min,
            predicted_throughput_pods_per_sec: predicted_throughput,
            formula: "sub_90s".to_string(),
            actual_gw_replicas: actual_gw,
            actual_wh_replicas: actual_wh,
        }
    }

    /// Compute the verdict by comparing prediction against actual result.
    #[must_use]
    pub fn verdict(&self, actual_duration_secs: f64) -> &'static str {
        if self.predicted_min_secs <= 0.0 || actual_duration_secs <= 0.0 {
            return "UNKNOWN";
        }
        let ratio = actual_duration_secs / self.predicted_min_secs;
        match ratio {
            r if r < 0.90 => "FASTER",
            r if r <= 1.10 => "ON_TARGET",
            r if r <= 1.50 => "SLOWER",
            _ => "UNDER_PROVISIONED",
        }
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

    #[test]
    fn prediction_cerebras_300() {
        let p = Prediction::calculate(300, 2, 5, 5, 3);
        assert_eq!(p.predicted_wh_replicas, 3);
        assert!(p.predicted_gw_replicas > 0);
        assert!(p.predicted_min_secs > 0.0);
        assert_eq!(p.actual_gw_replicas, 5);
        assert_eq!(p.actual_wh_replicas, 3);
    }

    #[test]
    fn prediction_cerebras_1000() {
        let p = Prediction::calculate(1000, 2, 5, 15, 5);
        assert_eq!(p.predicted_gw_replicas, 6); // ceil(1000*2/(5*67))
        assert_eq!(p.predicted_wh_replicas, 5);
    }

    #[test]
    fn verdict_on_target() {
        let p = Prediction::calculate(300, 2, 5, 5, 3);
        // If actual matches predicted within 10%
        let v = p.verdict(p.predicted_min_secs * 1.05);
        assert_eq!(v, "ON_TARGET");
    }

    #[test]
    fn verdict_faster() {
        let p = Prediction::calculate(300, 2, 5, 5, 3);
        let v = p.verdict(p.predicted_min_secs * 0.5);
        assert_eq!(v, "FASTER");
    }

    #[test]
    fn verdict_slower() {
        let p = Prediction::calculate(300, 2, 5, 5, 3);
        let v = p.verdict(p.predicted_min_secs * 1.3);
        assert_eq!(v, "SLOWER");
    }

    #[test]
    fn verdict_under_provisioned() {
        let p = Prediction::calculate(300, 2, 5, 5, 3);
        let v = p.verdict(p.predicted_min_secs * 2.0);
        assert_eq!(v, "UNDER_PROVISIONED");
    }
}
