//! Customer profile — maps a customer's Akeyless environment to test parameters.
//!
//! Profiles are portable descriptions of WHAT a customer needs. They are separate
//! from cluster bindings (WHERE to test) and experiment configs (HOW to test).

use serde::{Deserialize, Serialize};

/// Top-level customer profile loaded from YAML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomerProfile {
    pub customer: CustomerInfo,
    pub environment: EnvironmentSpec,
    pub workload: WorkloadSpec,
    pub akeyless: AkeylessSpec,
    #[serde(default)]
    pub constraints: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomerInfo {
    pub name: String,
    #[serde(default)]
    pub ticket: Option<String>,
    #[serde(default)]
    pub contacts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentSpec {
    pub nodes: u32,
    #[serde(default)]
    pub node_type: Option<String>,
    #[serde(default)]
    pub node_memory_gb: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkloadSpec {
    /// What the customer actually needs
    pub target_pods: u32,
    /// What we test to (typically 2-3x target for headroom validation)
    #[serde(default = "default_test_max")]
    pub test_max_pods: u32,
    #[serde(default = "default_secrets")]
    pub secrets_per_pod: u32,
    #[serde(default = "default_init")]
    pub init_containers: u32,
    #[serde(default)]
    pub workload_kind: WorkloadKind,
    #[serde(default)]
    pub restart_policy: Option<String>,
    #[serde(default)]
    pub pod_memory_gb: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkloadKind {
    #[default]
    Deployment,
    Job,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AkeylessSpec {
    #[serde(default = "default_auth")]
    pub auth_method: String,
    #[serde(default = "default_gw_nodes")]
    pub gateway_nodes: GatewayNodeMode,
    #[serde(default)]
    pub gateway_headroom_pct: Option<u32>,
    #[serde(default = "default_gw_memory")]
    pub gateway_memory: String,
    #[serde(default = "default_wh_timeout")]
    pub webhook_timeout_secs: u32,
    #[serde(default = "default_qps")]
    pub qps: u32,
    #[serde(default = "default_burst_qps")]
    pub burst_qps: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GatewayNodeMode {
    #[default]
    Shared,
    Dedicated,
}

fn default_test_max() -> u32 { 1000 }
fn default_secrets() -> u32 { 2 }
fn default_init() -> u32 { 1 }
fn default_auth() -> String { "k8s".to_string() }
fn default_gw_nodes() -> GatewayNodeMode { GatewayNodeMode::Shared }
fn default_gw_memory() -> String { "512Mi".to_string() }
fn default_wh_timeout() -> u32 { 30 }
fn default_qps() -> u32 { 5 }
fn default_burst_qps() -> u32 { 10 }

impl CustomerProfile {
    /// Load from a YAML file path.
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Failed to read profile {path}: {e}"))?;
        let profile: Self = serde_yaml::from_str(&content)
            .map_err(|e| anyhow::anyhow!("Failed to parse profile {path}: {e}"))?;
        profile.validate()?;
        Ok(profile)
    }

    /// Validate the profile constraints.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.workload.target_pods == 0 {
            anyhow::bail!("target_pods must be > 0");
        }
        if self.workload.test_max_pods < self.workload.target_pods {
            anyhow::bail!("test_max_pods must be >= target_pods");
        }
        if self.akeyless.qps == 0 {
            anyhow::bail!("qps must be > 0");
        }
        Ok(())
    }

    /// Calculate theoretical minimum injection time in seconds.
    pub fn theoretical_minimum_secs(&self, gw_replicas: u32) -> f64 {
        let total_requests = self.workload.test_max_pods * self.workload.secrets_per_pod;
        let aggregate_qps = gw_replicas * self.akeyless.qps;
        total_requests as f64 / aggregate_qps as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_profile_yaml() -> &'static str {
        r#"
customer:
  name: test-customer
  ticket: TEST-001
environment:
  nodes: 10
workload:
  target_pods: 100
  test_max_pods: 1000
  secrets_per_pod: 2
akeyless:
  qps: 5
"#
    }

    fn parse_profile(yaml: &str) -> CustomerProfile {
        serde_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn validate_passes_valid_profile() {
        let p = parse_profile(valid_profile_yaml());
        assert!(p.validate().is_ok());
    }

    #[test]
    fn validate_rejects_zero_target_pods() {
        let yaml = r#"
customer:
  name: test
environment:
  nodes: 10
workload:
  target_pods: 0
  test_max_pods: 100
akeyless:
  qps: 5
"#;
        let p = parse_profile(yaml);
        let err = p.validate().unwrap_err();
        assert!(err.to_string().contains("target_pods must be > 0"));
    }

    #[test]
    fn validate_rejects_test_max_less_than_target() {
        let yaml = r#"
customer:
  name: test
environment:
  nodes: 10
workload:
  target_pods: 200
  test_max_pods: 100
akeyless:
  qps: 5
"#;
        let p = parse_profile(yaml);
        let err = p.validate().unwrap_err();
        assert!(err.to_string().contains("test_max_pods must be >= target_pods"));
    }

    #[test]
    fn validate_rejects_zero_qps() {
        let yaml = r#"
customer:
  name: test
environment:
  nodes: 10
workload:
  target_pods: 100
  test_max_pods: 1000
akeyless:
  qps: 0
"#;
        let p = parse_profile(yaml);
        let err = p.validate().unwrap_err();
        assert!(err.to_string().contains("qps must be > 0"));
    }

    #[test]
    fn validate_passes_when_test_max_equals_target() {
        let yaml = r#"
customer:
  name: test
environment:
  nodes: 10
workload:
  target_pods: 100
  test_max_pods: 100
akeyless:
  qps: 5
"#;
        let p = parse_profile(yaml);
        assert!(p.validate().is_ok());
    }

    #[test]
    fn theoretical_minimum_secs_basic() {
        let p = parse_profile(valid_profile_yaml());
        let t = p.theoretical_minimum_secs(6);
        // total_requests = 1000 * 2 = 2000, aggregate_qps = 6 * 5 = 30
        // 2000 / 30 = 66.667
        assert!((t - 66.667).abs() < 0.01);
    }

    #[test]
    fn theoretical_minimum_secs_single_gw() {
        let p = parse_profile(valid_profile_yaml());
        let t = p.theoretical_minimum_secs(1);
        // 2000 / 5 = 400
        assert!((t - 400.0).abs() < f64::EPSILON);
    }

    #[test]
    fn serde_defaults_applied() {
        let yaml = r#"
customer:
  name: minimal
environment:
  nodes: 5
workload:
  target_pods: 50
akeyless: {}
"#;
        let p: CustomerProfile = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(p.workload.test_max_pods, 1000);
        assert_eq!(p.workload.secrets_per_pod, 2);
        assert_eq!(p.workload.init_containers, 1);
        assert_eq!(p.akeyless.auth_method, "k8s");
        assert_eq!(p.akeyless.gateway_memory, "512Mi");
        assert_eq!(p.akeyless.webhook_timeout_secs, 30);
        assert_eq!(p.akeyless.qps, 5);
        assert_eq!(p.akeyless.burst_qps, 10);
        assert!(p.constraints.is_empty());
    }

    #[test]
    fn workload_kind_default_is_deployment() {
        let yaml = r#"
customer:
  name: test
environment:
  nodes: 1
workload:
  target_pods: 10
akeyless: {}
"#;
        let p: CustomerProfile = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(p.workload.workload_kind, WorkloadKind::Deployment));
    }

    #[test]
    fn workload_kind_job_parses() {
        let yaml = r#"
customer:
  name: test
environment:
  nodes: 1
workload:
  target_pods: 10
  workload_kind: job
akeyless: {}
"#;
        let p: CustomerProfile = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(p.workload.workload_kind, WorkloadKind::Job));
    }

    #[test]
    fn gateway_node_mode_default_is_shared() {
        let yaml = r#"
customer:
  name: test
environment:
  nodes: 1
workload:
  target_pods: 10
akeyless: {}
"#;
        let p: CustomerProfile = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(p.akeyless.gateway_nodes, GatewayNodeMode::Shared));
    }

    #[test]
    fn gateway_node_mode_dedicated_parses() {
        let yaml = r#"
customer:
  name: test
environment:
  nodes: 1
workload:
  target_pods: 10
akeyless:
  gateway_nodes: dedicated
"#;
        let p: CustomerProfile = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(p.akeyless.gateway_nodes, GatewayNodeMode::Dedicated));
    }

    #[test]
    fn optional_fields_absent() {
        let yaml = r#"
customer:
  name: test
environment:
  nodes: 1
workload:
  target_pods: 10
akeyless: {}
"#;
        let p: CustomerProfile = serde_yaml::from_str(yaml).unwrap();
        assert!(p.customer.ticket.is_none());
        assert!(p.customer.contacts.is_empty());
        assert!(p.environment.node_type.is_none());
        assert!(p.environment.node_memory_gb.is_none());
        assert!(p.workload.restart_policy.is_none());
        assert!(p.workload.pod_memory_gb.is_none());
        assert!(p.akeyless.gateway_headroom_pct.is_none());
    }

    #[test]
    fn load_nonexistent_file_returns_error() {
        let err = CustomerProfile::load("/nonexistent/path/profile.yaml").unwrap_err();
        assert!(err.to_string().contains("Failed to read profile"));
    }
}
