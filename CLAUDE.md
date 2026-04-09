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
| `events.rs` | Structured event emission for Shinryū observability |
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
| `types.rs` | BurstResult, MatrixReport, PhaseTimings, PodDetail, WarmupTimings |

## Structured Events (Shinryū Integration)

burst-forge emits structured JSON events to stderr (captured by Vector
`kubernetes_logs` source) and optionally POSTs to a Vector HTTP endpoint.

| Event | Where Emitted | Payload |
|-------|--------------|---------|
| `MATRIX_START` | matrix.rs | scenario_count |
| `MATRIX_COMPLETE` | matrix.rs | scenario_count, passed, failed |
| `PHASE_COMPLETE` | matrix.rs (×3) | phase name, elapsed_ms |
| `POLL_TICK` | burst.rs (every 5th) | running, pending, failed, injected, elapsed_ms |
| `MILESTONE` | burst.rs | FIRST_READY, 50PCT_RUNNING, FULL_ADMISSION |
| `BURST_COMPLETE` | matrix.rs | Full BurstResult (20+ fields) |
| `SCENARIO_COMPLETE` | matrix.rs | success/failure, error message |
| `POD_STATE_DETAIL` | burst.rs | Notable pods: restart_count, state_reason, node |

Configure `vector_endpoint` in your config YAML to enable HTTP POST delivery:
```yaml
vector_endpoint: "http://vector.observability.svc:9500"
```

Events flow through Shinryū → Bronze NDJSON → Silver Parquet → queryable
via shinryu-mcp DataFusion SQL.

## Commands

| Command | What it does |
|---------|-------------|
| `flow <name>` | Run named flow from `configs/{name}.yaml` |
| `flow <name> --output json` | Same, with structured JSON output for agent consumption |
| `profile validate --profile X` | Validate a customer profile YAML |
| `profile show --profile X` | Show theoretical limits for a customer |
| `plan --profile X --cluster Y` | Generate 8-phase experiment plan from profile |
| `matrix` | Run matrix with explicit `--config` path |
| `burst` | Single burst (reset → warmup → execution) |
| `verify` | Check infrastructure readiness |
| `wait` | Poll FluxCD kustomizations |
| `reset` | Scale deployment to 0 |
| `reset-all` | Full teardown (deployment + pods + GW/WH + kustomizations + nodes) |
| `nodes` | EKS node group lifecycle |
| `report` | Publish JSON results to Confluence |

## Agent-Driven Experimentation

burst-forge supports autonomous agent-driven optimization via:

### Customer Profiles (`profiles/`)
Typed YAML describing a customer's environment (nodes, secrets, QPS constraints).
Separates WHAT the customer needs from HOW to test it.

### Cluster Bindings (`clusters/`)
Infrastructure-specific config (kubeconfig, node groups, Confluence).
Same customer profile can target different clusters.

### JSON Output Mode (`--output json`)
Structured NDJSON to stdout for machine consumption. Each lifecycle event
(phase, gate, burst, scenario) emits one JSON line. Terminal output unchanged.

### Shinryu SQL Templates (`queries/`)
Pre-built DataFusion SQL for post-experiment analysis:
- `gap-decomposition.sql` — WHERE does time go?
- `memory-pressure.sql` — GW heap at limit?
- `stall-detection.sql` — injection stall windows
- `cross-signal.sql` — asof_nearest memory↔stall correlation
- `dns-latency.sql` — Hubble DNS during burst
- `connection-reuse.sql` — TCP connection patterns

### 8-Phase Experiment Configs (`configs/phase{1-8}*.yaml`)
Full experiment suite for Cerebras optimization.

### Scaling Formulas (validated from 50+ experiments)
| Formula | Expression |
|---------|-----------|
| GW for sub-90s | `ceil(pods * secrets / (qps * 67))` |
| GW for sub-3min | `ceil(pods * secrets / (qps * 91))` |
| WH optimal (≤300) | 3 |
| WH optimal (≥500) | 5 |
| GW memory min | 768Mi (WH≤5), 1Gi (WH>5) |
| Theoretical floor | `(pods * secrets) / (gw * qps)` seconds |

### Validated GW Probe + Scaling Optimization (2026-04-08/09)

Four experiments at 10000 pods validated the optimal GW scaling config:

| Experiment | What | GW Scaling | Total Warmup | Result |
|------------|------|-----------|-------------|--------|
| Baseline | Chart defaults (60s delay, 5 batch) | 30 min (6 waves) | 50 min | Baseline |
| A+B+C | Probes + batch 10 + stabilize 30s | **6 min (3 waves)** | **23.5 min** | **Best** |
| D | Single-wave (all 30 at once) | 12.8 min (3 restarts) | 20.6 min | Viable |

**Recommended probe config** (validated, should be Helm chart default):
```yaml
startupProbe:
  httpGet: {path: /health, port: 8080}
  initialDelaySeconds: 15    # was 60 — GW serves at ~20s
  periodSeconds: 5           # was 10
  failureThreshold: 12       # 60s max startup window

readinessProbe:
  httpGet: {path: /health, port: 8080}
  initialDelaySeconds: 0     # startupProbe handles slow starts
  periodSeconds: 5
  timeoutSeconds: 5
```

**Recommended burst-forge config:**
```yaml
gateway_batch_size: 10              # 3 waves for 30 pods (was 5 → 6 waves)
post_scale_stabilize_secs: 30      # was 180 — probes ensure readiness
burst_batch_size: 2500              # webhook admission capacity headroom
```

**Why batched (A+B+C) beats single-wave (D):**
- 3/30 pods fail startup probe at 60s when all start simultaneously
- ClusterCache leader takes ~20s; 29 followers compete for Redis reads
- Batched waves give each cohort exclusive CPU + Redis bandwidth

### Requirements by Scale (validated)
| Pods | GW | WH | Burst Nodes | GW Nodes | Subnets |
|------|----|----|-------------|----------|---------|
| 1000 | 12 | 7 | 18 | 3 | /20 |
| 2000 | 12 | 12 | 36 | 3 | /18 |
| 5000 | 21 | 16 | 87 | 6 | /18 |
| 10000 | 30 | 16 | 174 | 8 | /18 |

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
