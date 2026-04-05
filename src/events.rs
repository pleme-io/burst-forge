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
}
