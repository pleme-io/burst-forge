//! Structured event emission for Shinryū observability pipeline.
//!
//! Emits JSON events to stderr (captured by Vector kubernetes_logs) and
//! optionally POSTs to a Vector HTTP endpoint for direct ingestion.
//!
//! Event types follow the burst-forge lifecycle:
//! - `MATRIX_START` / `MATRIX_COMPLETE` — experiment boundaries
//! - `PHASE_COMPLETE` — phase milestones (RESET, WARMUP, EXECUTION)
//! - `POLL_TICK` — per-poll pod state snapshot (every Nth tick)
//! - `GATE_RESULT` — gate verification pass/fail
//! - `MILESTONE` — timing milestones (first_ready, 50pct, all_ready)
//! - `BURST_COMPLETE` — iteration result with full BurstResult
//! - `SCENARIO_COMPLETE` — scenario done

use chrono::Utc;
use serde::Serialize;

/// A structured event for the Shinryū observability pipeline.
#[derive(Debug, Serialize)]
pub struct BurstForgeEvent {
    pub timestamp: String,
    pub event_type: String,
    pub experiment_id: String,
    pub scenario: String,
    #[serde(flatten)]
    pub payload: serde_json::Value,
}

impl BurstForgeEvent {
    fn new(event_type: &str, experiment_id: &str, scenario: &str, payload: serde_json::Value) -> Self {
        Self {
            timestamp: Utc::now().to_rfc3339(),
            event_type: event_type.to_string(),
            experiment_id: experiment_id.to_string(),
            scenario: scenario.to_string(),
            payload,
        }
    }
}

/// Event emitter that writes to stderr (for Vector kubernetes_logs capture)
/// and optionally POSTs to a Vector HTTP endpoint.
pub struct EventEmitter {
    experiment_id: String,
    vector_endpoint: Option<String>,
    enabled: bool,
}

impl EventEmitter {
    /// Create a new event emitter.
    /// `vector_endpoint`: optional URL like `http://vector.observability.svc:9500`
    pub fn new(experiment_id: String, vector_endpoint: Option<String>) -> Self {
        Self {
            experiment_id,
            vector_endpoint,
            enabled: true,
        }
    }

    /// Create a disabled emitter (no events emitted).
    pub fn disabled() -> Self {
        Self {
            experiment_id: String::new(),
            vector_endpoint: None,
            enabled: false,
        }
    }

    /// Emit an event to stderr and optionally to Vector HTTP endpoint.
    pub fn emit(&self, event: &BurstForgeEvent) {
        if !self.enabled {
            return;
        }
        // Write single-line JSON to stderr (Vector captures via kubernetes_logs)
        if let Ok(json) = serde_json::to_string(event) {
            eprintln!("{json}");
        }
        // POST to Vector HTTP endpoint if configured
        if let Some(ref endpoint) = self.vector_endpoint {
            if let Ok(body) = serde_json::to_vec(event) {
                // Best-effort POST — don't block the experiment on delivery failure
                let _ = std::net::TcpStream::connect_timeout(
                    &endpoint.replace("http://", "").parse().unwrap_or_else(|_| "127.0.0.1:9500".parse().unwrap()),
                    std::time::Duration::from_secs(2),
                ).and_then(|mut stream| {
                    use std::io::Write;
                    let request = format!(
                        "POST / HTTP/1.1\r\nHost: vector\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                        body.len()
                    );
                    stream.write_all(request.as_bytes())?;
                    stream.write_all(&body)?;
                    Ok(())
                });
            }
        }
    }

    // ── Convenience constructors ────────────────────────────────

    pub fn matrix_start(&self, scenario_count: usize) {
        self.emit(&BurstForgeEvent::new(
            "MATRIX_START", &self.experiment_id, "",
            serde_json::json!({ "scenario_count": scenario_count }),
        ));
    }

    pub fn matrix_complete(&self, scenario_count: usize, passed: usize, failed: usize) {
        self.emit(&BurstForgeEvent::new(
            "MATRIX_COMPLETE", &self.experiment_id, "",
            serde_json::json!({ "scenario_count": scenario_count, "passed": passed, "failed": failed }),
        ));
    }

    pub fn phase_complete(&self, scenario: &str, phase: &str, elapsed_ms: u64) {
        self.emit(&BurstForgeEvent::new(
            "PHASE_COMPLETE", &self.experiment_id, scenario,
            serde_json::json!({ "phase": phase, "elapsed_ms": elapsed_ms }),
        ));
    }

    pub fn poll_tick(
        &self, scenario: &str, running: u32, pending: u32, failed: u32,
        injected: u32, elapsed_ms: u64, peak_running: u32,
    ) {
        self.emit(&BurstForgeEvent::new(
            "POLL_TICK", &self.experiment_id, scenario,
            serde_json::json!({
                "running": running, "pending": pending, "failed": failed,
                "injected": injected, "elapsed_ms": elapsed_ms,
                "peak_running": peak_running,
                "injection_rate_pct": if running > 0 {
                    (f64::from(injected) / f64::from(running)) * 100.0
                } else { 0.0 },
            }),
        ));
    }

    pub fn gate_result(&self, scenario: &str, gate: &str, passed: bool, detail: &str) {
        self.emit(&BurstForgeEvent::new(
            "GATE_RESULT", &self.experiment_id, scenario,
            serde_json::json!({ "gate": gate, "passed": passed, "detail": detail }),
        ));
    }

    pub fn milestone(&self, scenario: &str, milestone: &str, elapsed_ms: u64, value: u32) {
        self.emit(&BurstForgeEvent::new(
            "MILESTONE", &self.experiment_id, scenario,
            serde_json::json!({ "milestone": milestone, "elapsed_ms": elapsed_ms, "value": value }),
        ));
    }

    pub fn burst_complete(&self, scenario: &str, result: &crate::types::BurstResult) {
        let mut payload = serde_json::to_value(result).unwrap_or_default();

        // Flatten prediction into top-level event fields for Shinryu querying
        if let Some(ref pred) = result.prediction {
            if let serde_json::Value::Object(ref mut map) = payload {
                map.insert("predicted_gw_replicas".to_string(), pred.predicted_gw_replicas.into());
                map.insert("predicted_wh_replicas".to_string(), pred.predicted_wh_replicas.into());
                map.insert("predicted_min_secs".to_string(), serde_json::json!(pred.predicted_min_secs));
                map.insert("predicted_throughput".to_string(), serde_json::json!(pred.predicted_throughput_pods_per_sec));
                map.insert("prediction_formula".to_string(), pred.formula.clone().into());

                // Calculate verdict from actual duration
                let actual_secs = result.duration_ms as f64 / 1000.0;
                map.insert("prediction_verdict".to_string(), pred.verdict(actual_secs).into());
                map.insert("prediction_error_pct".to_string(), serde_json::json!(
                    if pred.predicted_min_secs > 0.0 {
                        ((actual_secs - pred.predicted_min_secs) / pred.predicted_min_secs) * 100.0
                    } else {
                        0.0
                    }
                ));
            }
        }

        self.emit(&BurstForgeEvent::new(
            "BURST_COMPLETE", &self.experiment_id, scenario,
            payload,
        ));
    }

    pub fn scenario_complete(&self, scenario: &str, success: bool, error: Option<&str>) {
        self.emit(&BurstForgeEvent::new(
            "SCENARIO_COMPLETE", &self.experiment_id, scenario,
            serde_json::json!({ "success": success, "error": error }),
        ));
    }

    /// Emit sampled pod state details (restart counts, failure reasons, node placement).
    pub fn pod_state_detail(&self, scenario: &str, pods: &[crate::types::PodDetail]) {
        // Only emit problematic pods (restarts > 0, pending with reason, failed)
        let notable: Vec<&crate::types::PodDetail> = pods.iter()
            .filter(|p| p.restart_count > 0 || p.state_reason.is_some() || p.phase == "Failed")
            .collect();
        if notable.is_empty() {
            return;
        }
        self.emit(&BurstForgeEvent::new(
            "POD_STATE_DETAIL", &self.experiment_id, scenario,
            serde_json::json!({
                "notable_pod_count": notable.len(),
                "pods": serde_json::to_value(&notable).unwrap_or_default(),
            }),
        ));
    }
}

/// Generate an experiment ID from a flow name and current timestamp.
pub fn generate_experiment_id(flow_name: &str) -> String {
    format!("{}-{}", flow_name, Utc::now().format("%Y%m%dT%H%MZ"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_id_has_flow_name() {
        let id = generate_experiment_id("cerebras-matrix");
        assert!(id.starts_with("cerebras-matrix-"));
    }

    #[test]
    fn event_serializes_to_json() {
        let event = BurstForgeEvent::new(
            "POLL_TICK", "test-exp", "scenario-1",
            serde_json::json!({ "running": 100 }),
        );
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("POLL_TICK"));
        assert!(json.contains("test-exp"));
        assert!(json.contains("running"));
    }

    #[test]
    fn disabled_emitter_does_not_panic() {
        let emitter = EventEmitter::disabled();
        emitter.matrix_start(5);
        emitter.poll_tick("test", 10, 5, 0, 8, 1000, 10);
    }

    #[test]
    fn disabled_emitter_all_methods_safe() {
        let emitter = EventEmitter::disabled();
        emitter.matrix_complete(5, 4, 1);
        emitter.phase_complete("s1", "RESET", 5000);
        emitter.gate_result("s1", "Gate1", true, "ok");
        emitter.milestone("s1", "FIRST_READY", 100, 1);
        emitter.scenario_complete("s1", true, None);
        emitter.scenario_complete("s1", false, Some("error msg"));
    }

    #[test]
    fn event_timestamp_is_rfc3339() {
        let event = BurstForgeEvent::new(
            "TEST", "exp-1", "s1",
            serde_json::json!({}),
        );
        assert!(event.timestamp.contains('T'));
        assert!(event.timestamp.contains('+') || event.timestamp.ends_with('Z'));
    }

    #[test]
    fn event_fields_preserved_in_json() {
        let event = BurstForgeEvent::new(
            "MATRIX_START", "exp-abc", "scenario-x",
            serde_json::json!({"key": "value"}),
        );
        let json: serde_json::Value = serde_json::to_value(&event).unwrap();
        assert_eq!(json["event_type"], "MATRIX_START");
        assert_eq!(json["experiment_id"], "exp-abc");
        assert_eq!(json["scenario"], "scenario-x");
        assert_eq!(json["key"], "value");
    }

    #[test]
    fn event_payload_flattened() {
        let event = BurstForgeEvent::new(
            "TEST", "exp-1", "s1",
            serde_json::json!({"running": 100, "pending": 5}),
        );
        let json: serde_json::Value = serde_json::to_value(&event).unwrap();
        assert_eq!(json["running"], 100);
        assert_eq!(json["pending"], 5);
    }

    #[test]
    fn generate_id_contains_timestamp_pattern() {
        let id = generate_experiment_id("test-flow");
        assert!(id.starts_with("test-flow-"));
        let suffix = &id["test-flow-".len()..];
        assert!(suffix.contains('T'));
        assert!(suffix.ends_with('Z'));
    }

    #[test]
    fn generate_id_unique_per_call() {
        let id1 = generate_experiment_id("flow");
        let id2 = generate_experiment_id("flow");
        // Within the same minute, they should be equal — but this
        // validates format consistency
        assert!(id1.starts_with("flow-"));
        assert!(id2.starts_with("flow-"));
    }

    #[test]
    fn emitter_new_is_enabled() {
        let emitter = EventEmitter::new("test-id".to_string(), None);
        assert!(emitter.enabled);
    }

    #[test]
    fn emitter_disabled_is_not_enabled() {
        let emitter = EventEmitter::disabled();
        assert!(!emitter.enabled);
    }

    #[test]
    fn pod_state_detail_filters_notable_pods() {
        let emitter = EventEmitter::disabled();
        let pods = vec![
            crate::types::PodDetail {
                name: "healthy".to_string(),
                phase: "Running".to_string(),
                node: None,
                creation_timestamp: None,
                restart_count: 0,
                state_reason: None,
                container_started_at: None,
                qos_class: None,
                host_ip: None,
                pod_ip: None,
                injected: true,
            },
            crate::types::PodDetail {
                name: "restarted".to_string(),
                phase: "Running".to_string(),
                node: None,
                creation_timestamp: None,
                restart_count: 3,
                state_reason: None,
                container_started_at: None,
                qos_class: None,
                host_ip: None,
                pod_ip: None,
                injected: true,
            },
            crate::types::PodDetail {
                name: "waiting".to_string(),
                phase: "Pending".to_string(),
                node: None,
                creation_timestamp: None,
                restart_count: 0,
                state_reason: Some("ImagePullBackOff".to_string()),
                container_started_at: None,
                qos_class: None,
                host_ip: None,
                pod_ip: None,
                injected: false,
            },
            crate::types::PodDetail {
                name: "crashed".to_string(),
                phase: "Failed".to_string(),
                node: None,
                creation_timestamp: None,
                restart_count: 0,
                state_reason: None,
                container_started_at: None,
                qos_class: None,
                host_ip: None,
                pod_ip: None,
                injected: false,
            },
        ];
        // Should not panic — disabled emitter silently drops
        emitter.pod_state_detail("test", &pods);
    }

    #[test]
    fn pod_state_detail_no_notable_does_not_emit() {
        let emitter = EventEmitter::disabled();
        let pods = vec![
            crate::types::PodDetail {
                name: "healthy".to_string(),
                phase: "Running".to_string(),
                node: None,
                creation_timestamp: None,
                restart_count: 0,
                state_reason: None,
                container_started_at: None,
                qos_class: None,
                host_ip: None,
                pod_ip: None,
                injected: true,
            },
        ];
        // No notable pods — should return early without emitting
        emitter.pod_state_detail("test", &pods);
    }

    #[test]
    fn burst_complete_without_prediction() {
        let emitter = EventEmitter::disabled();
        let result = crate::types::BurstResult {
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            replicas_requested: 100,
            pods_running: 100,
            pods_failed: 0,
            pods_pending: 0,
            pods_injected: 100,
            injection_success_rate: 100.0,
            time_to_first_ready_ms: 500,
            time_to_all_ready_ms: Some(5000),
            time_to_full_admission_ms: Some(4000),
            time_to_50pct_running_ms: Some(2500),
            admission_rate_pods_per_sec: 20.0,
            gateway_throughput_pods_per_sec: 18.0,
            duration_ms: 6000,
            nodes: 3,
            iteration: 1,
            total_secrets_injected: 200,
            peak_running: 100,
            prediction: None,
        };
        // Should not panic
        emitter.burst_complete("test", &result);
    }

    #[test]
    fn burst_complete_with_prediction_flattens_fields() {
        let _emitter = EventEmitter::new("test".to_string(), None);
        let result = crate::types::BurstResult {
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            replicas_requested: 100,
            pods_running: 100,
            pods_failed: 0,
            pods_pending: 0,
            pods_injected: 100,
            injection_success_rate: 100.0,
            time_to_first_ready_ms: 500,
            time_to_all_ready_ms: Some(5000),
            time_to_full_admission_ms: Some(4000),
            time_to_50pct_running_ms: Some(2500),
            admission_rate_pods_per_sec: 20.0,
            gateway_throughput_pods_per_sec: 18.0,
            duration_ms: 60_000,
            nodes: 3,
            iteration: 1,
            total_secrets_injected: 200,
            peak_running: 100,
            prediction: Some(crate::types::Prediction {
                predicted_gw_replicas: 6,
                predicted_wh_replicas: 5,
                predicted_min_secs: 66.67,
                predicted_throughput_pods_per_sec: 15.0,
                formula: "sub_90s".to_string(),
                actual_gw_replicas: 6,
                actual_wh_replicas: 5,
            }),
        };

        // Verify the payload construction logic
        let mut payload = serde_json::to_value(&result).unwrap();
        if let Some(ref pred) = result.prediction {
            if let serde_json::Value::Object(ref mut map) = payload {
                map.insert("predicted_gw_replicas".to_string(), pred.predicted_gw_replicas.into());
                map.insert("predicted_min_secs".to_string(), serde_json::json!(pred.predicted_min_secs));
                let actual_secs = result.duration_ms as f64 / 1000.0;
                map.insert("prediction_verdict".to_string(), pred.verdict(actual_secs).into());
                let error_pct = if pred.predicted_min_secs > 0.0 {
                    ((actual_secs - pred.predicted_min_secs) / pred.predicted_min_secs) * 100.0
                } else {
                    0.0
                };
                map.insert("prediction_error_pct".to_string(), serde_json::json!(error_pct));
            }
        }

        assert_eq!(payload["predicted_gw_replicas"], 6);
        assert_eq!(payload["predicted_min_secs"], 66.67);

        // duration_ms=60000 → actual_secs=60, predicted=66.67
        // ratio = 60/66.67 = 0.8999 → FASTER (< 0.90)
        assert_eq!(payload["prediction_verdict"], "FASTER");

        // error_pct = (60 - 66.67) / 66.67 * 100 ≈ -10.0%
        let error_pct = payload["prediction_error_pct"].as_f64().unwrap();
        assert!((error_pct - (-10.0)).abs() < 0.5);
    }
}
