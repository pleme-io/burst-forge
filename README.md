# burst-forge

Kubernetes burst test orchestrator -- coordinated pod scaling with configurable
injection verification. Generic (no vendor hardcoding): works with any secret
injection system (Akeyless, Vault, etc.) via YAML config.

## Architecture

burst-forge uses a **3-phase lifecycle** with **5 explicit gates** to ensure
deterministic, reproducible burst tests.

### 3-Phase Lifecycle

```
Phase 1: RESET      Scale to 0, force-delete pods, verify zero state
Phase 2: WARMUP     Verify nodes, image cache, gateway, webhook rollout
Phase 3: EXECUTION  Scale 0 -> N, poll, measure timing + injection rate
```

Each phase is independently timed. Phase timings are included in every burst
result, making it clear how much time is spent on reset/warmup vs. actual burst.

### 5-Gate System

Gates are explicit pass/fail checks that must pass before the next phase starts.
Each gate produces a `GateResult` with expected vs. actual state for diagnostics.

| Gate | Phase | Checks |
|------|-------|--------|
| Zero State | Reset -> Warmup | 0 pods running, deployment at 0 replicas |
| Node Ready | Warmup | All burst nodes Ready + Schedulable |
| Image Cache | Warmup | Zot pods running, required images cached |
| Infra Ready | Warmup | Gateway + webhook at target replica count |
| Warmup Complete | Warmup -> Execution | DaemonSet rollout complete, all images pre-pulled |

Gates can be strict (fail the run) or advisory (warn and continue) via config.

## Modules

| Module | Purpose |
|--------|---------|
| `config` | Shikumi-powered YAML config discovery + deserialization |
| `kubectl` | kubectl subprocess wrapper with kubeconfig support |
| `flux` | FluxCD kustomization polling (wait for Ready) |
| `verify` | Infrastructure readiness checks (nodes, gateway, webhook, cache) |
| `burst` | Core burst logic: scale 0->N, poll pods, measure injection |
| `matrix` | Scaling matrix orchestrator: scenarios, HelmRelease patching |
| `nodes` | EKS node group lifecycle (scale up/down, status, tagging) |
| `drain` | Pod drain + force-delete, deployment replica queries |
| `gates` | 5-gate system with pass/fail diagnostics |
| `phases` | 3-phase lifecycle (Reset, Warmup, Execution) with timing |
| `report` | Confluence XHTML report generation + publishing via API |
| `output` | Terminal UI: banners, progress, color, signal handler output |
| `types` | Shared types: BurstResult, MatrixReport, PhaseTimings |

## Commands

| Command | Description |
|---------|-------------|
| `verify` | Check infrastructure readiness: nodes, gateway, webhook, image cache |
| `wait` | Poll FluxCD kustomizations until all reach `Ready` status |
| `burst` | Scale 0 -> N, measure timing and injection rate. Supports `--replicas` and `--iterations` |
| `matrix` | Run all configured scaling scenarios. Patches HelmRelease replicas between scenarios |
| `reset` | Scale the target deployment back to 0 replicas |
| `reset-all` | Full environment reset: deployment to 0, drain pods, gateway/webhook to 1, resume HelmReleases, nodes to 0 |
| `nodes up/down/status` | Manage EKS node group for burst testing |
| `report` | Publish a JSON results file to Confluence |

### Signal Handler (Ctrl+C)

Ctrl+C triggers graceful cleanup at any point:

1. Scales deployment to 0
2. Force-deletes all burst pods (`--grace-period=0 --force`)
3. Waits briefly for drain
4. Scales node group to 0

No orphaned pods or costly nodes left running after interruption.

## Configuration

Config discovery (shikumi pattern):

1. `--config <path>` CLI flag
2. `BURST_FORGE_CONFIG` env var
3. `~/.config/burst-forge/burst-forge.yaml`
4. Defaults (no config file needed for basic usage)

### Config Format

```yaml
namespace: scale-test
deployment: nginx-burst
timeout_secs: 600
poll_interval_secs: 5
cooldown_secs: 15
rollout_wait_secs: 120

# Pod drain configuration
drain_timeout_secs: 120
drain_poll_interval_secs: 5

# Injection detection mode: "sidecar" (2+ containers) or "env" (env var prefix)
injection_mode: env
injection_env_prefix: "AKEYLESS_"

# Image cache (Zot registry)
image_cache:
  namespace: image-cache
  label: "app.kubernetes.io/name=zot"
  registry: "image-cache.image-cache.svc.cluster.local:5000"

required_images:
  - "akeyless/k8s-secrets-sidecar:0.35.1"
  - "library/nginx:1.27-alpine"

# FluxCD kustomizations to wait for
flux:
  namespace: flux-system
  kustomizations:
    - infrastructure-image-cache
    - infrastructure-injection
    - scale-test-workloads

# Injection infrastructure (configure for your system)
injection_namespace: akeyless-system
gateway_deployment: akeyless-gateway-akeyless-api-gateway
webhook_deployment: akeyless-secrets-injection
gateway_release: akeyless-gateway
webhook_release: akeyless-secrets-injection

# EKS node group (burst-forge manages lifecycle)
node_group:
  cluster_name: scale-test
  nodegroup_name: scale-test-burst
  region: us-east-1
  aws_profile: my-aws-profile
  pods_per_node: 58
  max_nodes: 20

# DaemonSet warmup -- wait for image pre-pull after node scale-up
warmup_daemonset:
  namespace: scale-test
  name: image-warmup
  timeout_secs: 300

# Confluence auto-publish (API token from CONFLUENCE_API_TOKEN env var)
confluence:
  base_url: "myorg.atlassian.net"
  space_key: "MYSPACE"
  parent_page_id: "12345"
  user_email: "user@example.com"

# Scaling matrix -- scenarios run largest first
scenarios:
  - name: "1000-pods"
    replicas: 1000
    gateway_replicas: 6
    webhook_replicas: 7

  - name: "300-pods"
    replicas: 300
    gateway_replicas: 2
    webhook_replicas: 3

  - name: "50-pods"
    replicas: 50
    gateway_replicas: 1
    webhook_replicas: 1
```

See `examples/burst-forge.yaml` for a complete configuration with all scenario tiers.

## Injection Detection

Two modes based on how your injection system adds secrets:

- **`env` mode** (default): Webhook injects environment variables (e.g., `AKEYLESS_TOKEN`) into existing containers. No extra sidecar. Detected by checking env vars with configured prefix.
- **`sidecar` mode**: Webhook adds a sidecar container. Detected by container count >= 2.

Set `injection_mode` in config or `--injection-mode` on CLI.

## Matrix Orchestration

The `matrix` command is the top-level orchestrator:

1. Reads scenarios from YAML config (runs largest first for maximum pressure)
2. Scales EKS node group based on largest scenario's pod count
3. Waits for FluxCD kustomizations in dependency order
4. Validates image cache populated
5. For each scenario:
   - Patches `maxSurge` to match replica count (simultaneous pod creation)
   - Patches HelmRelease gateway/webhook replicas
   - Waits for rollout complete
   - Runs 3-phase burst (Reset -> Warmup -> Execution)
   - Collects JSON results
6. Outputs full JSON report
7. Publishes to Confluence (if configured)
8. Scales nodes back to 0

### maxSurge Patching

burst-forge patches `maxSurge` on the target deployment to match the scenario's
replica count before each burst. This ensures all pods are created simultaneously,
putting maximum concurrent pressure on the injection infrastructure.

## Output

All commands output structured JSON. Burst results include phase timings:

```json
{
  "timestamp": "2026-03-30T...",
  "replicas_requested": 300,
  "pods_running": 300,
  "pods_failed": 0,
  "pods_pending": 0,
  "injection_success_rate": 93.7,
  "time_to_first_ready_ms": 2340,
  "time_to_all_ready_ms": 45200,
  "duration_ms": 50500,
  "nodes": 8,
  "iteration": 1,
  "phase_timings": {
    "reset_ms": 3200,
    "warmup_ms": 12400,
    "execution_ms": 34900
  }
}
```

Matrix reports aggregate results across all scenarios with Confluence auto-publishing.

## Usage

```sh
# Verify infrastructure is ready
burst-forge verify

# Wait for FluxCD to reconcile all kustomizations
burst-forge wait

# Run a single burst test (50 pods, 1 iteration)
burst-forge burst --replicas 50

# Run a burst test with 3 iterations (results averaged)
burst-forge burst --replicas 100 --iterations 3

# Run the full scaling matrix
burst-forge matrix

# Run a single scenario from the matrix
burst-forge matrix --scenario "300-pods"

# Full environment reset (deployment, pods, gateway, webhook, nodes)
burst-forge reset-all

# Manage nodes
burst-forge nodes up --count 18
burst-forge nodes status
burst-forge nodes down

# Publish results to Confluence
burst-forge report --json results.json

# Use a specific kubeconfig
burst-forge --kubeconfig /path/to/kubeconfig verify
```

## Build

Built with substrate's `rust-tool-release-flake.nix`:

```sh
nix build    # local binary
nix run      # run directly
```
