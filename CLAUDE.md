# burst-forge

Kubernetes burst test orchestrator. Generic (no vendor hardcoding) -- works with
any secret injection system via YAML config.

## Build & Run

```bash
cargo build --release         # fastest for iteration
nix run .#cerebras-matrix     # full Cerebras battery (50-1000, 2 secrets)
nix run .#optimized-matrix    # optimized GW counts (50-1000)
nix run .#original-matrix     # original table GW counts (50-1000)
nix run .#quick-1000          # single 1000-pod experiment
```

Single scenario from a matrix: `nix run .#cerebras-matrix -- --scenario cerebras-300`

## Architecture

3-phase lifecycle with 5 explicit gates:

```
Phase 1: RESET      -> Gate: Zero State
Phase 2: WARMUP     -> Gates: Node Ready, Image Cache, Infra Ready, Warmup Complete
Phase 3: EXECUTION  -> burst 0->N, poll, measure
```

## Module Map

| Module | Purpose |
|--------|---------|
| `config.rs` | Shikumi YAML config discovery + serde types |
| `kubectl.rs` | kubectl subprocess wrapper |
| `flux.rs` | FluxCD kustomization polling |
| `verify.rs` | Infrastructure readiness checks |
| `burst.rs` | Core burst: scale 0->N, poll, measure injection |
| `matrix.rs` | Scaling matrix: scenarios, HelmRelease patching, node lifecycle |
| `nodes.rs` | EKS node group scale up/down/status (AWS CLI) |
| `drain.rs` | Pod drain, force-delete, deployment replica queries |
| `gates.rs` | 5-gate system with GateResult diagnostics |
| `phases.rs` | 3-phase lifecycle with per-phase timing |
| `report.rs` | Confluence XHTML report generation + REST API publish |
| `output.rs` | Terminal UI: banners, progress, color, signal handler |
| `types.rs` | BurstResult, MatrixReport, PhaseTimings, WarmupTimings |

## Commands

| Command | What it does |
|---------|-------------|
| `verify` | Nodes + gateway + webhook + image cache |
| `wait` | FluxCD kustomization polling |
| `burst` | 3-phase burst (reset -> warmup -> execution) |
| `matrix` | Full scaling matrix with node/HelmRelease orchestration |
| `reset` | Deployment to 0 replicas |
| `reset-all` | Full env reset (deployment + pods + gateway + webhook + nodes) |
| `nodes up/down/status` | EKS node group lifecycle |
| `report` | Publish JSON results to Confluence |

## Experiment Cycle

See `docs/experiment-cycle.md` for the full methodology. Quick reference:

1. Identify bottleneck from previous run output
2. Hypothesize ONE fix, predict effect
3. Document on Confluence BEFORE running
4. Apply change, run `burst-forge matrix --scenario "1000-pods"`
5. Measure, report, iterate

## Current Constraints (ASM-17583)

- **QPS = 5 req/s per GW replica** (permanent, no env var fix)
- **11 GW replicas** needed for 1000-pod burst (55 req/s aggregate)
- **4 worker nodes** (t3.medium) for infrastructure pods
- **19 burst nodes** (m5.xlarge) for workload pods
- **Agent CPU reduced** from 250m to 25m to unlock scheduling

## Config Location

```
--config flag > BURST_FORGE_CONFIG env > ~/.config/burst-forge/burst-forge.yaml
```

Example: `examples/burst-forge.yaml`

## Key Design Decisions

1. **Generic injection** -- `injection_mode: env` or `sidecar`, no vendor-specific code
2. **Shikumi config** -- standard discovery (`~/.config/burst-forge/burst-forge.yaml`)
3. **Signal handler** -- Ctrl+C cleans up deployment + pods + nodes (no orphans)
4. **Gates are explicit** -- pass/fail with expected/actual diagnostics
5. **Phase timing** -- reset/warmup/execution independently measured
6. **Confluence auto-publish** -- matrix results published as XHTML tables
7. **maxSurge patching** -- ensures all pods created simultaneously per scenario
8. **postRenderers for nodeSelector** -- Akeyless charts don't support nodeSelector in values
