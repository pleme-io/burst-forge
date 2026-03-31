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
    pub duration_ms: u64,
    pub nodes: u32,
    pub iteration: u32,
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

/// Result of a single scaling scenario.
#[derive(Debug, Serialize, Deserialize)]
pub struct ScenarioResult {
    pub name: String,
    pub replicas: u32,
    pub gateway_replicas: u32,
    pub webhook_replicas: u32,
    pub verify: Option<VerifyResult>,
    pub burst: Option<BurstResult>,
    pub error: Option<String>,
}

/// Aggregated results from the scaling matrix.
#[derive(Debug, Serialize, Deserialize)]
pub struct MatrixReport {
    pub timestamp: String,
    pub scenarios: Vec<ScenarioResult>,
}
