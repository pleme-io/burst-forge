//! Typed shigoto planning surface for burst-forge.
//!
//! Foundation step toward full shigoto adoption per `theory/SHIGOTO.md`.
//! This module maps burst-forge's matrix concepts onto typed `JobId` /
//! `JobKindId` / `Dag` primitives — pure data transformation, no
//! execution change. Legacy `phases.rs` still drives `matrix::run_matrix`;
//! consumers can build a Dag from the same config to inspect the planned
//! work graph, and future PRs incrementally migrate phase execution onto
//! `shigoto-scheduler`.
//!
//! Closes the planning half of SHIGOTO.md §V.1 criterion 1 — second
//! production consumer (tend is the first, post-M0.22). Full execution
//! migration is tracked in CLAUDE.md "shigoto migration" section.

// Foundation-only — full execution lands in follow-up PRs that implement
// the Job trait per phase and call shigoto-scheduler. Marking the
// surface dead-code-allowed keeps the warning surface clean until the
// executor PR lands; see CLAUDE.md "shigoto migration" section.
#![allow(dead_code)]

use shigoto_dag::Dag;
use shigoto_types::{JobId, JobKindId, JobScope, JobSubject};

use crate::config::Config;

/// JobKindId for Phase 1: reset workload to verified zero state.
pub const RESET_KIND: &str = "burst-forge.reset";

/// JobKindId for Phase 2: warmup all infrastructure to ready state
/// (nodes, image-warmup DaemonSet, IPAMD, injection infra, gates).
pub const WARMUP_KIND: &str = "burst-forge.warmup";

/// JobKindId for Phase 3: execute the burst (scale 0→N, measure
/// injection success).
pub const EXECUTION_KIND: &str = "burst-forge.execution";

/// Build a typed Dag of JobIds for the given config's scenario matrix.
///
/// Each scenario produces three Jobs (reset → warmup → execution) chained
/// by edges. Independent scenarios are wave-parallel; their reset Jobs
/// share no dependency, so a future scheduler with budget>1 could run
/// them concurrently. The current legacy executor in `matrix::run_matrix`
/// runs scenarios sequentially.
///
/// The returned Dag is suitable for `toposort()` / `waves()` to inspect
/// the plan, and as the input shape that a future
/// `shigoto-scheduler::InProcessScheduler::execute_dag` call will consume
/// once execute hooks land.
#[must_use]
pub fn plan_dag(config: &Config) -> Dag {
    let mut dag = Dag::new();
    let scope = workspace_scope(config);

    for scenario in &config.scenarios {
        let reset = JobId {
            scope: scope.clone(),
            kind: JobKindId::new(RESET_KIND),
            subject: JobSubject::Pinned(scenario.name.clone()),
        };
        let warmup = JobId {
            scope: scope.clone(),
            kind: JobKindId::new(WARMUP_KIND),
            subject: JobSubject::Pinned(scenario.name.clone()),
        };
        let execution = JobId {
            scope: scope.clone(),
            kind: JobKindId::new(EXECUTION_KIND),
            subject: JobSubject::Pinned(scenario.name.clone()),
        };

        // Per-scenario chain: reset → warmup → execution.
        dag.add_edge(reset.clone(), warmup.clone());
        dag.add_edge(warmup, execution);
        // Ensure isolated single-scenario configs still get a node for
        // reset (add_edge already inserts; the explicit ensure is a
        // no-op when reset already has outgoing edges).
        dag.ensure_node(reset);
    }

    dag
}

/// Workspace-scope identity for burst-forge JobIds. burst-forge always
/// targets one cluster + namespace per invocation; we use the cluster
/// kubectl context as the workspace identifier so JobIds are stable
/// across invocations of the same matrix against the same cluster.
fn workspace_scope(config: &Config) -> JobScope {
    JobScope::Workspace(format!(
        "{namespace}@{deployment}",
        namespace = config.namespace,
        deployment = config.deployment,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_job(scenario: &str, kind: &str) -> JobId {
        JobId {
            scope: JobScope::Workspace("scale-test@burst-target".into()),
            kind: JobKindId::new(kind),
            subject: JobSubject::Pinned(scenario.into()),
        }
    }

    #[test]
    fn kind_ids_are_distinct() {
        assert_ne!(RESET_KIND, WARMUP_KIND);
        assert_ne!(WARMUP_KIND, EXECUTION_KIND);
        assert_ne!(RESET_KIND, EXECUTION_KIND);
    }

    #[test]
    fn manual_dag_three_phase_chain() {
        // Validates the same edge-pattern plan_dag emits, without
        // depending on Config / Scenario struct construction.
        let mut dag = Dag::new();
        let r = make_job("s50", RESET_KIND);
        let w = make_job("s50", WARMUP_KIND);
        let e = make_job("s50", EXECUTION_KIND);
        dag.add_edge(r.clone(), w.clone());
        dag.add_edge(w, e);
        dag.ensure_node(r);
        let ordered = dag.toposort().expect("acyclic");
        assert_eq!(ordered.len(), 3);
        assert_eq!(ordered[0].kind.0, RESET_KIND);
        assert_eq!(ordered[1].kind.0, WARMUP_KIND);
        assert_eq!(ordered[2].kind.0, EXECUTION_KIND);
        for job in &ordered {
            assert_eq!(job.subject, JobSubject::Pinned("s50".into()));
        }
    }

    #[test]
    fn two_scenarios_six_jobs_independent() {
        let mut dag = Dag::new();
        for s in &["s50", "s100"] {
            dag.add_edge(make_job(s, RESET_KIND), make_job(s, WARMUP_KIND));
            dag.add_edge(make_job(s, WARMUP_KIND), make_job(s, EXECUTION_KIND));
        }
        let ordered = dag.toposort().expect("acyclic");
        assert_eq!(ordered.len(), 6);
        // First wave (no deps): both scenarios' reset jobs.
        let waves = dag.waves(None).expect("acyclic");
        assert_eq!(waves.len(), 3);
        assert_eq!(waves[0].len(), 2); // two parallel resets
        assert_eq!(waves[1].len(), 2); // two parallel warmups
        assert_eq!(waves[2].len(), 2); // two parallel executions
    }
}
