# burst-forge

Kubernetes burst test orchestrator. Generic (no vendor hardcoding) -- works with
any secret injection system via YAML config.

## Build & Run

```bash
cargo build --release              # build
burst-forge flow cerebras-matrix   # full Cerebras battery (50-1000, 2 secrets)
burst-forge flow optimized-matrix  # optimized GW counts (50-1000)
burst-forge flow original-matrix   # original table GW counts (50-1000)
burst-forge flow single-1000       # quick 1000-pod experiment
```

Single scenario: `burst-forge flow cerebras-matrix --scenario cerebras-300`

## Flow Pattern

The `flow` subcommand is the standard way to run experiments. It discovers
`configs/{name}.yaml` and handles the full lifecycle — no manual steps:

1. Suspend kustomizations (from config `suspend_kustomizations`)
2. Scale burst nodes + wait for IPAMD warmup (from config `ipamd_warmup_secs`)
3. Scale GW/WH per scenario
4. Burst 0→N, measure injection
5. Inter-scenario drain + cooldown
6. Cleanup: resume kustomizations, scale nodes to 0, resume HelmReleases

Configs are self-contained YAML — kubeconfig, scenarios, infrastructure params,
Confluence publishing, IPAMD warmup, kustomization management. All in one file.

**Private configs** (with Confluence keys, access IDs, customer details) live in
`pleme-io/k8s` (private repo), not here.

## Architecture

3-phase lifecycle with 5 explicit gates:

```
Phase 1: RESET      -> Gate: Zero State
Phase 2: WARMUP     -> Gates: Node Ready, Image Warmup, IPAMD Warmup, Infra Ready
Phase 3: EXECUTION  -> burst 0->N, poll, measure
```

## Module Map

| Module | Purpose |
|--------|---------|
| `config.rs` | Shikumi YAML config + flow discovery |
| `kubectl.rs` | kubectl subprocess wrapper |
| `flux.rs` | FluxCD kustomization polling |
| `verify.rs` | Infrastructure readiness checks |
| `burst.rs` | Core burst: scale 0->N, poll, measure injection |
| `matrix.rs` | Scaling matrix: scenarios, HelmRelease patching, node lifecycle |
| `nodes.rs` | EKS node group scale up/down/status |
| `drain.rs` | Pod drain, force-delete, deployment replica queries |
| `gates.rs` | 5-gate system with GateResult diagnostics |
| `phases.rs` | 3-phase lifecycle with per-phase timing |
| `report.rs` | Confluence XHTML report generation + REST API publish |
| `output.rs` | Terminal UI: banners, progress, color, signal handler |
| `types.rs` | BurstResult, MatrixReport, PhaseTimings, WarmupTimings |

## Commands

| Command | What it does |
|---------|-------------|
| `flow <name>` | Run named flow from `configs/{name}.yaml` |
| `matrix` | Run matrix with explicit `--config` path |
| `burst` | Single burst (reset → warmup → execution) |
| `verify` | Check infrastructure readiness |
| `wait` | Poll FluxCD kustomizations |
| `reset` | Scale deployment to 0 |
| `reset-all` | Full teardown (deployment + pods + GW/WH + kustomizations + nodes) |
| `nodes` | EKS node group lifecycle |
| `report` | Publish JSON results to Confluence |

## Key Config Fields

| Field | Purpose | Default |
|-------|---------|---------|
| `kubeconfig` | Path to kubeconfig (~ expanded) | CLI `--kubeconfig` |
| `ipamd_warmup_secs` | Sleep after Gate 2 for ENI attachment | 0 |
| `suspend_kustomizations` | FluxCD kustomizations to suspend/resume | [] |
| `scenarios[]` | Scaling tiers (replicas, GW, WH) | required |
| `confluence` | Auto-publish results | optional |

## Key Design Decisions

1. **Rust consumes YAML** -- no shell orchestration, all logic in Rust
2. **Generic injection** -- `injection_mode: env` or `sidecar`, no vendor code
3. **Shikumi config** -- standard discovery pattern
4. **Signal handler** -- Ctrl+C resumes kustomizations + scales nodes to 0
5. **Gates are explicit** -- pass/fail with expected/actual diagnostics
6. **Phase timing** -- reset/warmup/execution independently measured
7. **Confluence auto-publish** -- matrix results as XHTML tables
