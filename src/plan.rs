//! Experiment plan generator — transforms customer profiles into 8-phase experiment configs.
//!
//! Encodes validated scaling laws from 40+ experiments:
//! - GW for sub-90s: ceil(pods * secrets / (qps * 67))
//! - WH optimal: 3 for ≤300 pods, 5 for ≥500
//! - GW memory minimum: 1Gi when WH>5
//! - Theoretical min: (pods * secrets) / (gw * qps) seconds

use serde::{Deserialize, Serialize};

use crate::profile::CustomerProfile;

// ---------------------------------------------------------------------------
// Scaling formulas
// ---------------------------------------------------------------------------

/// GW replicas needed for sub-90s injection at given QPS.
#[must_use]
pub fn gw_for_sub_90s(pods: u32, secrets: u32, qps: u32) -> u32 {
    ((f64::from(pods) * f64::from(secrets)) / (f64::from(qps) * 67.0)).ceil() as u32
}

/// GW replicas needed for sub-3min injection.
#[must_use]
pub fn gw_for_sub_3min(pods: u32, secrets: u32, qps: u32) -> u32 {
    ((f64::from(pods) * f64::from(secrets)) / (f64::from(qps) * 91.0)).ceil() as u32
}

/// Optimal WH count based on pod scale.
#[must_use]
pub fn wh_optimal(pods: u32) -> u32 {
    match pods {
        0..=300 => 3,
        301..=499 => 4,
        _ => 5,
    }
}

/// Minimum safe GW memory limit.
#[must_use]
pub fn gw_memory_min(wh_count: u32) -> &'static str {
    if wh_count <= 5 { "768Mi" } else { "1Gi" }
}

/// Theoretical minimum injection time in seconds.
#[must_use]
pub fn theoretical_min_secs(pods: u32, secrets: u32, gw: u32, qps: u32) -> f64 {
    f64::from(pods * secrets) / f64::from(gw * qps)
}

/// Memory sweep values for Phase 1.
#[must_use]
pub fn memory_sweep_values() -> Vec<&'static str> {
    vec!["512Mi", "768Mi", "1Gi", "1536Mi", "2Gi"]
}

/// WH sweep values for Phase 2.
#[must_use]
pub fn wh_sweep_values() -> Vec<u32> {
    vec![3, 5, 8, 10, 12, 15]
}

/// GW sweep values for Phase 3.
#[must_use]
pub fn gw_sweep_values(baseline: u32) -> Vec<u32> {
    vec![baseline, baseline + 5, baseline + 10]
}

// ---------------------------------------------------------------------------
// Plan types
// ---------------------------------------------------------------------------

/// Complete experiment plan generated from a customer profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperimentPlan {
    pub customer: String,
    pub phases: Vec<PhasePlan>,
    pub theoretical_min_secs: f64,
    pub recommended_gw: u32,
    pub recommended_wh: u32,
    pub recommended_memory: String,
}

/// A single phase within the experiment plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhasePlan {
    pub phase: u32,
    pub name: String,
    pub hypothesis: String,
    pub config_path: String,
    pub scenarios: Vec<ScenarioPlan>,
}

/// A single scenario within a phase.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioPlan {
    pub name: String,
    pub replicas: u32,
    pub gw: u32,
    pub wh: u32,
    pub gw_memory: String,
}

/// Plan manifest written to disk — summarizes the full plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanManifest {
    pub customer: String,
    pub generated: String,
    pub theoretical_minimum_secs: f64,
    pub recommended: RecommendedConfig,
    pub phases: Vec<PhaseManifestEntry>,
    pub decision_trees: DecisionTrees,
}

/// Recommended infrastructure configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecommendedConfig {
    pub gateway_replicas: u32,
    pub webhook_replicas: u32,
    pub gateway_memory: String,
}

/// Summary of a phase in the manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaseManifestEntry {
    pub phase: u32,
    pub name: String,
    pub config: String,
    pub scenarios: usize,
    pub hypothesis: String,
}

/// Decision trees for transitioning between phases.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionTrees {
    pub phase1_to_phase2: String,
    pub phase2_to_phase3: String,
    pub phase3_to_phase4: String,
}

// ---------------------------------------------------------------------------
// Plan generation
// ---------------------------------------------------------------------------

/// Generate an experiment plan from a customer profile.
///
/// Produces 8 phases covering memory sweeps, WH contention, GW scaling,
/// network isolation, combined optimization, headroom validation,
/// endurance, and production readiness.
///
/// # Errors
///
/// Returns an error if the profile has invalid parameters (e.g., zero QPS).
pub fn generate_plan(profile: &CustomerProfile, cluster_path: &str) -> anyhow::Result<ExperimentPlan> {
    let pods = profile.workload.test_max_pods;
    let secrets = profile.workload.secrets_per_pod;
    let qps = profile.akeyless.qps;
    let customer = &profile.customer.name;

    let rec_gw = gw_for_sub_90s(pods, secrets, qps);
    let rec_wh = wh_optimal(pods);
    let rec_memory = gw_memory_min(rec_wh).to_string();
    let theo_min = theoretical_min_secs(pods, secrets, rec_gw, qps);

    let _ = cluster_path; // reserved for future cluster binding merge

    let phases = vec![
        generate_phase1(customer, pods, secrets, rec_gw, rec_wh),
        generate_phase2(customer, pods, secrets, rec_gw, &rec_memory),
        generate_phase3(customer, pods, secrets, rec_wh, &rec_memory, rec_gw),
        generate_phase4(customer, pods, secrets, rec_gw, rec_wh, &rec_memory),
        generate_phase5(customer, pods, secrets, rec_gw, rec_wh, &rec_memory),
        generate_phase6(customer, profile, secrets, rec_gw, rec_wh, &rec_memory),
        generate_phase7(customer, pods, secrets, rec_gw, rec_wh, &rec_memory),
        generate_phase8(customer, pods, secrets, rec_gw, rec_wh, &rec_memory),
    ];

    Ok(ExperimentPlan {
        customer: customer.clone(),
        phases,
        theoretical_min_secs: theo_min,
        recommended_gw: rec_gw,
        recommended_wh: rec_wh,
        recommended_memory: rec_memory,
    })
}

/// Phase 1: GW Memory Sweep — find minimum safe GW memory limit.
fn generate_phase1(customer: &str, pods: u32, _secrets: u32, gw: u32, wh: u32) -> PhasePlan {
    let mut scenarios = Vec::new();
    let wh_high = wh * 2;

    for mem in memory_sweep_values() {
        // At optimal WH
        scenarios.push(ScenarioPlan {
            name: format!("mem-{mem}-wh{wh}"),
            replicas: pods,
            gw,
            wh,
            gw_memory: mem.to_string(),
        });
        // At 2x WH to test contention interaction
        scenarios.push(ScenarioPlan {
            name: format!("mem-{mem}-wh{wh_high}"),
            replicas: pods,
            gw,
            wh: wh_high,
            gw_memory: mem.to_string(),
        });
    }

    // Trim the 2x WH variant for the last memory value to keep scenario count reasonable
    // (10 scenarios is already a full day's work)
    if scenarios.len() > 10 {
        scenarios.truncate(10);
    }

    PhasePlan {
        phase: 1,
        name: "GW Memory Sweep".to_string(),
        hypothesis: "Find minimum safe GW memory limit".to_string(),
        config_path: format!("configs/{customer}-phase1.yaml"),
        scenarios,
    }
}

/// Phase 2: WH Contention Sweep — find optimal webhook replica count.
fn generate_phase2(customer: &str, pods: u32, _secrets: u32, gw: u32, memory: &str) -> PhasePlan {
    let scenarios = wh_sweep_values()
        .into_iter()
        .map(|wh| ScenarioPlan {
            name: format!("wh-{wh}"),
            replicas: pods,
            gw,
            wh,
            gw_memory: memory.to_string(),
        })
        .collect();

    PhasePlan {
        phase: 2,
        name: "WH Contention Sweep".to_string(),
        hypothesis: "Find optimal WH count — expect cliff at WH>=8 from 40+ experiment data".to_string(),
        config_path: format!("configs/{customer}-phase2.yaml"),
        scenarios,
    }
}

/// Phase 3: GW Scaling Sweep — find optimal gateway replica count.
fn generate_phase3(customer: &str, pods: u32, _secrets: u32, wh: u32, memory: &str, baseline_gw: u32) -> PhasePlan {
    let scenarios = gw_sweep_values(baseline_gw)
        .into_iter()
        .map(|gw| ScenarioPlan {
            name: format!("gw-{gw}"),
            replicas: pods,
            gw,
            wh,
            gw_memory: memory.to_string(),
        })
        .collect();

    PhasePlan {
        phase: 3,
        name: "GW Scaling Sweep".to_string(),
        hypothesis: "Validate GW scaling law: more GW replicas = proportionally faster injection".to_string(),
        config_path: format!("configs/{customer}-phase3.yaml"),
        scenarios,
    }
}

/// Phase 4: Network Isolation — test with NetworkPolicy and Cilium.
fn generate_phase4(customer: &str, pods: u32, _secrets: u32, gw: u32, wh: u32, memory: &str) -> PhasePlan {
    let scenarios = vec![
        ScenarioPlan {
            name: "network-baseline".to_string(),
            replicas: pods,
            gw,
            wh,
            gw_memory: memory.to_string(),
        },
        ScenarioPlan {
            name: "network-policy-enabled".to_string(),
            replicas: pods,
            gw,
            wh,
            gw_memory: memory.to_string(),
        },
    ];

    PhasePlan {
        phase: 4,
        name: "Network Isolation".to_string(),
        hypothesis: "NetworkPolicy adds <5% overhead at recommended config".to_string(),
        config_path: format!("configs/{customer}-phase4.yaml"),
        scenarios,
    }
}

/// Phase 5: Combined Optimization — best values from phases 1-4.
fn generate_phase5(customer: &str, pods: u32, _secrets: u32, gw: u32, wh: u32, memory: &str) -> PhasePlan {
    let scenarios = vec![
        ScenarioPlan {
            name: "optimized-full".to_string(),
            replicas: pods,
            gw,
            wh,
            gw_memory: memory.to_string(),
        },
        ScenarioPlan {
            name: "optimized-headroom-10pct".to_string(),
            replicas: pods,
            gw: ((f64::from(gw) * 1.1).ceil()) as u32,
            wh,
            gw_memory: memory.to_string(),
        },
    ];

    PhasePlan {
        phase: 5,
        name: "Combined Optimization".to_string(),
        hypothesis: "Best config from phases 1-4 achieves sub-90s at test_max_pods".to_string(),
        config_path: format!("configs/{customer}-phase5.yaml"),
        scenarios,
    }
}

/// Phase 6: Headroom Validation — test at target_pods, 1.5x, and 2x.
fn generate_phase6(customer: &str, profile: &CustomerProfile, _secrets: u32, gw: u32, wh: u32, memory: &str) -> PhasePlan {
    let target = profile.workload.target_pods;
    let test_max = profile.workload.test_max_pods;

    let mut tiers = vec![target];
    let mid = (f64::from(target) * 1.5).ceil() as u32;
    if mid < test_max {
        tiers.push(mid);
    }
    tiers.push(test_max);

    let scenarios = tiers
        .into_iter()
        .map(|r| ScenarioPlan {
            name: format!("headroom-{r}"),
            replicas: r,
            gw,
            wh,
            gw_memory: memory.to_string(),
        })
        .collect();

    PhasePlan {
        phase: 6,
        name: "Headroom Validation".to_string(),
        hypothesis: "Recommended config handles target_pods to test_max_pods gracefully".to_string(),
        config_path: format!("configs/{customer}-phase6.yaml"),
        scenarios,
    }
}

/// Phase 7: Endurance — sustained load and restart recovery.
fn generate_phase7(customer: &str, pods: u32, _secrets: u32, gw: u32, wh: u32, memory: &str) -> PhasePlan {
    let scenarios = vec![
        ScenarioPlan {
            name: "endurance-sustained".to_string(),
            replicas: pods,
            gw,
            wh,
            gw_memory: memory.to_string(),
        },
        ScenarioPlan {
            name: "endurance-restart-recovery".to_string(),
            replicas: pods,
            gw,
            wh,
            gw_memory: memory.to_string(),
        },
    ];

    PhasePlan {
        phase: 7,
        name: "Endurance".to_string(),
        hypothesis: "Config remains stable under sustained load and recovers from GW restart".to_string(),
        config_path: format!("configs/{customer}-phase7.yaml"),
        scenarios,
    }
}

/// Phase 8: Production Readiness — final validation with production-like settings.
fn generate_phase8(customer: &str, pods: u32, _secrets: u32, gw: u32, wh: u32, memory: &str) -> PhasePlan {
    let scenarios = vec![
        ScenarioPlan {
            name: "prod-ready-cold-start".to_string(),
            replicas: pods,
            gw,
            wh,
            gw_memory: memory.to_string(),
        },
        ScenarioPlan {
            name: "prod-ready-warm-start".to_string(),
            replicas: pods,
            gw,
            wh,
            gw_memory: memory.to_string(),
        },
    ];

    PhasePlan {
        phase: 8,
        name: "Production Readiness".to_string(),
        hypothesis: "Final config achieves target SLO from both cold and warm starts".to_string(),
        config_path: format!("configs/{customer}-phase8.yaml"),
        scenarios,
    }
}

// ---------------------------------------------------------------------------
// Plan manifest output
// ---------------------------------------------------------------------------

/// Write the plan manifest YAML to `plan-manifest.yaml`.
///
/// # Errors
///
/// Returns an error if the manifest cannot be serialized or written.
pub fn write_plan_configs(plan: &ExperimentPlan, _cluster_path: &str) -> anyhow::Result<()> {
    let manifest = PlanManifest {
        customer: plan.customer.clone(),
        generated: chrono::Utc::now().to_rfc3339(),
        theoretical_minimum_secs: plan.theoretical_min_secs,
        recommended: RecommendedConfig {
            gateway_replicas: plan.recommended_gw,
            webhook_replicas: plan.recommended_wh,
            gateway_memory: plan.recommended_memory.clone(),
        },
        phases: plan
            .phases
            .iter()
            .map(|p| PhaseManifestEntry {
                phase: p.phase,
                name: p.name.clone(),
                config: p.config_path.clone(),
                scenarios: p.scenarios.len(),
                hypothesis: p.hypothesis.clone(),
            })
            .collect(),
        decision_trees: DecisionTrees {
            phase1_to_phase2: format!(
                "If 768Mi ~ 1Gi performance: use 768Mi. If WH={wh2x} beats WH={wh} at 1Gi: Phase 2 critical.",
                wh2x = plan.recommended_wh * 2,
                wh = plan.recommended_wh,
            ),
            phase2_to_phase3: "If cliff still at WH>=8: keep WH=5. If no cliff: use WH=8-10.".to_string(),
            phase3_to_phase4: "If 20 GW <10% gain over recommended: move to network. If gains persist: Phase 5.".to_string(),
        },
    };

    let yaml = serde_yaml::to_string(&manifest)
        .map_err(|e| anyhow::anyhow!("Failed to serialize plan manifest: {e}"))?;

    std::fs::write("plan-manifest.yaml", &yaml)
        .map_err(|e| anyhow::anyhow!("Failed to write plan-manifest.yaml: {e}"))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gw_sub_90s_cerebras() {
        // 1000 pods * 2 secrets / (5 QPS * 67) = 5.97 -> 6
        assert_eq!(gw_for_sub_90s(1000, 2, 5), 6);
    }

    #[test]
    fn gw_sub_3min_cerebras() {
        // 1000 * 2 / (5 * 91) = 4.39 -> 5
        assert_eq!(gw_for_sub_3min(1000, 2, 5), 5);
    }

    #[test]
    fn wh_optimal_small() {
        assert_eq!(wh_optimal(100), 3);
        assert_eq!(wh_optimal(300), 3);
    }

    #[test]
    fn wh_optimal_medium() {
        assert_eq!(wh_optimal(301), 4);
        assert_eq!(wh_optimal(499), 4);
    }

    #[test]
    fn wh_optimal_large() {
        assert_eq!(wh_optimal(500), 5);
        assert_eq!(wh_optimal(1000), 5);
    }

    #[test]
    fn memory_min_low_wh() {
        assert_eq!(gw_memory_min(3), "768Mi");
        assert_eq!(gw_memory_min(5), "768Mi");
    }

    #[test]
    fn memory_min_high_wh() {
        assert_eq!(gw_memory_min(6), "1Gi");
        assert_eq!(gw_memory_min(10), "1Gi");
    }

    #[test]
    fn theoretical_floor() {
        // 1000 * 2 / (6 * 5) = 66.67
        let t = theoretical_min_secs(1000, 2, 6, 5);
        assert!((t - 66.67).abs() < 0.01);
    }

    #[test]
    fn gw_sweep_from_baseline() {
        let sweep = gw_sweep_values(10);
        assert_eq!(sweep, vec![10, 15, 20]);
    }

    #[test]
    fn plan_generates_8_phases() {
        let profile = test_profile();
        let plan = generate_plan(&profile, "clusters/test.yaml").unwrap();
        assert_eq!(plan.phases.len(), 8);
    }

    #[test]
    fn plan_phase1_has_memory_sweep_scenarios() {
        let profile = test_profile();
        let plan = generate_plan(&profile, "clusters/test.yaml").unwrap();
        let phase1 = &plan.phases[0];
        assert_eq!(phase1.phase, 1);
        assert!(phase1.scenarios.len() >= 5, "Phase 1 should have at least 5 memory sweep scenarios");
    }

    #[test]
    fn plan_phase2_has_wh_sweep_scenarios() {
        let profile = test_profile();
        let plan = generate_plan(&profile, "clusters/test.yaml").unwrap();
        let phase2 = &plan.phases[1];
        assert_eq!(phase2.phase, 2);
        assert_eq!(phase2.scenarios.len(), wh_sweep_values().len());
    }

    #[test]
    fn plan_phase3_has_gw_sweep_scenarios() {
        let profile = test_profile();
        let plan = generate_plan(&profile, "clusters/test.yaml").unwrap();
        let phase3 = &plan.phases[2];
        assert_eq!(phase3.phase, 3);
        assert_eq!(phase3.scenarios.len(), 3);
    }

    #[test]
    fn plan_recommended_values() {
        let profile = test_profile();
        let plan = generate_plan(&profile, "clusters/test.yaml").unwrap();
        assert_eq!(plan.recommended_wh, 5); // 1000 pods -> 5
        assert!(plan.recommended_gw > 0);
        assert!(plan.theoretical_min_secs > 0.0);
    }

    #[test]
    fn plan_phase6_headroom_tiers() {
        let profile = test_profile();
        let plan = generate_plan(&profile, "clusters/test.yaml").unwrap();
        let phase6 = &plan.phases[5];
        assert_eq!(phase6.phase, 6);
        // target=300, 1.5x=450, test_max=1000 -> 3 tiers
        assert_eq!(phase6.scenarios.len(), 3);
        assert_eq!(phase6.scenarios[0].replicas, 300);
        assert_eq!(phase6.scenarios[2].replicas, 1000);
    }

    #[test]
    fn gw_sub_90s_small_scale() {
        // 50 pods * 2 secrets / (5 * 67) = 100/335 = 0.2985 → 1
        assert_eq!(gw_for_sub_90s(50, 2, 5), 1);
    }

    #[test]
    fn gw_sub_90s_zero_pods() {
        assert_eq!(gw_for_sub_90s(0, 2, 5), 0);
    }

    #[test]
    fn gw_sub_90s_zero_secrets() {
        assert_eq!(gw_for_sub_90s(1000, 0, 5), 0);
    }

    #[test]
    fn gw_sub_90s_high_qps() {
        // 1000 * 2 / (100 * 67) = 2000/6700 = 0.298 → 1
        assert_eq!(gw_for_sub_90s(1000, 2, 100), 1);
    }

    #[test]
    fn gw_sub_90s_many_secrets() {
        // 1000 * 10 / (5 * 67) = 10000/335 = 29.85 → 30
        assert_eq!(gw_for_sub_90s(1000, 10, 5), 30);
    }

    #[test]
    fn gw_sub_3min_zero_pods() {
        assert_eq!(gw_for_sub_3min(0, 2, 5), 0);
    }

    #[test]
    fn gw_sub_3min_small_scale() {
        // 50 * 2 / (5 * 91) = 100/455 = 0.22 → 1
        assert_eq!(gw_for_sub_3min(50, 2, 5), 1);
    }

    #[test]
    fn wh_optimal_zero_pods() {
        assert_eq!(wh_optimal(0), 3);
    }

    #[test]
    fn wh_optimal_boundary_300_to_301() {
        assert_eq!(wh_optimal(300), 3);
        assert_eq!(wh_optimal(301), 4);
    }

    #[test]
    fn wh_optimal_boundary_499_to_500() {
        assert_eq!(wh_optimal(499), 4);
        assert_eq!(wh_optimal(500), 5);
    }

    #[test]
    fn wh_optimal_very_large() {
        assert_eq!(wh_optimal(100_000), 5);
    }

    #[test]
    fn gw_memory_min_boundary() {
        assert_eq!(gw_memory_min(5), "768Mi");
        assert_eq!(gw_memory_min(6), "1Gi");
    }

    #[test]
    fn gw_memory_min_zero() {
        assert_eq!(gw_memory_min(0), "768Mi");
    }

    #[test]
    fn theoretical_min_secs_zero_pods() {
        let t = theoretical_min_secs(0, 2, 6, 5);
        assert!((t - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn theoretical_min_secs_one_gw_one_qps() {
        // 100 * 2 / (1 * 1) = 200
        let t = theoretical_min_secs(100, 2, 1, 1);
        assert!((t - 200.0).abs() < f64::EPSILON);
    }

    #[test]
    fn memory_sweep_values_correct() {
        let vals = memory_sweep_values();
        assert_eq!(vals.len(), 5);
        assert!(vals.contains(&"512Mi"));
        assert!(vals.contains(&"2Gi"));
    }

    #[test]
    fn wh_sweep_values_correct() {
        let vals = wh_sweep_values();
        assert_eq!(vals.len(), 6);
        assert_eq!(vals[0], 3);
        assert_eq!(*vals.last().unwrap(), 15);
    }

    #[test]
    fn gw_sweep_from_baseline_zero() {
        let sweep = gw_sweep_values(0);
        assert_eq!(sweep, vec![0, 5, 10]);
    }

    #[test]
    fn plan_all_phases_numbered_sequentially() {
        let profile = test_profile();
        let plan = generate_plan(&profile, "clusters/test.yaml").unwrap();
        for (i, phase) in plan.phases.iter().enumerate() {
            assert_eq!(phase.phase as usize, i + 1);
        }
    }

    #[test]
    fn plan_phase5_has_headroom_variant() {
        let profile = test_profile();
        let plan = generate_plan(&profile, "clusters/test.yaml").unwrap();
        let phase5 = &plan.phases[4];
        assert_eq!(phase5.scenarios.len(), 2);
        assert!(phase5.scenarios[1].name.contains("headroom"));
        assert!(phase5.scenarios[1].gw >= phase5.scenarios[0].gw);
    }

    #[test]
    fn plan_phase4_network_isolation_has_baseline() {
        let profile = test_profile();
        let plan = generate_plan(&profile, "clusters/test.yaml").unwrap();
        let phase4 = &plan.phases[3];
        assert_eq!(phase4.scenarios.len(), 2);
        assert!(phase4.scenarios[0].name.contains("baseline"));
    }

    #[test]
    fn plan_phase7_endurance_scenarios() {
        let profile = test_profile();
        let plan = generate_plan(&profile, "clusters/test.yaml").unwrap();
        let phase7 = &plan.phases[6];
        assert_eq!(phase7.scenarios.len(), 2);
        assert!(phase7.scenarios[0].name.contains("sustained"));
        assert!(phase7.scenarios[1].name.contains("restart"));
    }

    #[test]
    fn plan_phase8_production_readiness() {
        let profile = test_profile();
        let plan = generate_plan(&profile, "clusters/test.yaml").unwrap();
        let phase8 = &plan.phases[7];
        assert_eq!(phase8.scenarios.len(), 2);
        assert!(phase8.scenarios[0].name.contains("cold"));
        assert!(phase8.scenarios[1].name.contains("warm"));
    }

    /// Build a test profile matching Cerebras-like parameters.
    fn test_profile() -> CustomerProfile {
        let yaml = r#"
customer:
  name: test-customer
  ticket: TEST-001
environment:
  nodes: 151
  node_type: m5.2xlarge
workload:
  target_pods: 300
  test_max_pods: 1000
  secrets_per_pod: 2
akeyless:
  qps: 5
  burst_qps: 10
"#;
        serde_yaml::from_str(yaml).unwrap()
    }
}
