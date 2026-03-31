# burst-forge

Kubernetes burst test orchestrator — coordinated pod scaling with configurable
injection verification. Generic (no vendor hardcoding): works with any secret
injection system (Akeyless, Vault, etc.) via YAML config.

## Usage

```bash
# Named flow — discovers configs/{name}.yaml, handles full lifecycle
burst-forge flow cerebras-matrix

# Single scenario from a flow
burst-forge flow cerebras-matrix --scenario cerebras-300

# Explicit config path
burst-forge matrix --config path/to/config.yaml

# Infrastructure commands
burst-forge verify
burst-forge nodes status
burst-forge reset-all
```

## Flow Pattern

The `flow` subcommand is the standard way to run experiments. Given a name,
it discovers `configs/{name}.yaml` and handles the complete lifecycle:

1. Suspend FluxCD kustomizations (from config `suspend_kustomizations`)
2. Scale EKS burst nodes + wait for IPAMD warmup (from config `ipamd_warmup_secs`)
3. For each scenario: scale GW/WH, verify gates, burst 0→N, measure, drain
4. Publish results to Confluence
5. Cleanup: resume kustomizations, scale nodes to 0, resume HelmReleases

Zero manual steps. Config YAML is the source of truth.

## Architecture

### 3-Phase Lifecycle

```
Phase 1: RESET      Scale to 0, drain pods, verify zero state
Phase 2: WARMUP     Nodes → Images → IPAMD → Gateway → Webhook → Gates
Phase 3: EXECUTION  Scale 0→N, poll injection, measure timing
```

Each phase is independently timed. Phase timings are in every result.

### 5-Gate System

| Gate | Phase | Checks |
|------|-------|--------|
| Zero State | Reset → Warmup | 0 pods, deployment at 0 |
| Node Ready | Warmup | All burst nodes Ready + Schedulable |
| Image Warmup | Warmup | DaemonSet rollout complete |
| Infra Ready | Warmup | GW + WH at target replica count |
| Starting Line | Warmup → Execution | 0 pods, GW/WH healthy |

Gates are explicit pass/fail with expected vs. actual diagnostics.

## Configuration

Config discovery:

1. `flow <name>` → `configs/{name}.yaml`
2. `--config <path>` → explicit path
3. `BURST_FORGE_CONFIG` env var
4. `~/.config/burst-forge/burst-forge.yaml`

### Config Format

```yaml
kubeconfig: "~/.kube/my-cluster.yaml"
namespace: scale-test
deployment: nginx-burst
timeout_secs: 600
ipamd_warmup_secs: 150

# Kustomizations to suspend during burst (prevents GitOps replica revert)
suspend_kustomizations:
  - scale-test-workloads

# Injection detection
injection_mode: env
injection_env_prefix: "AKEYLESS_"

# Injection infrastructure
injection_namespace: akeyless-system
gateway_deployment: akeyless-gateway
webhook_deployment: akeyless-secrets-injection
gateway_release: akeyless-gateway
webhook_release: akeyless-secrets-injection

# EKS node group (burst-forge manages lifecycle)
node_group:
  cluster_name: my-cluster
  nodegroup_name: my-burst-nodes
  region: us-east-1
  pods_per_node: 58
  max_nodes: 20

# DaemonSet warmup for image pre-pull
warmup_daemonset:
  namespace: scale-test
  name: image-warmup
  timeout_secs: 300

# Confluence auto-publish
confluence:
  base_url: "myorg.atlassian.net"
  space_key: "ENG"
  parent_page_id: "12345"
  user_email: "user@example.com"

# Scaling matrix (largest first for maximum pressure)
scenarios:
  - name: "1000-pods"
    replicas: 1000
    gateway_replicas: 11
    webhook_replicas: 7
  - name: "300-pods"
    replicas: 300
    gateway_replicas: 4
    webhook_replicas: 3
  - name: "50-pods"
    replicas: 50
    gateway_replicas: 1
    webhook_replicas: 1
```

## Commands

| Command | Description |
|---------|-------------|
| `flow <name>` | Run named flow from `configs/{name}.yaml` |
| `matrix` | Run scaling matrix with `--config` path |
| `burst` | Single burst test (0→N) |
| `verify` | Check infrastructure readiness |
| `wait` | Poll FluxCD kustomizations |
| `reset` | Scale deployment to 0 |
| `reset-all` | Full teardown (deployment, pods, GW/WH, kustomizations, nodes) |
| `nodes up/down/status` | EKS node group lifecycle |
| `report` | Publish JSON results to Confluence |

## Injection Detection

- **`env` mode** (default): Webhook injects env vars (e.g., `AKEYLESS_TOKEN`). Detected by prefix match.
- **`sidecar` mode**: Webhook adds a sidecar container. Detected by container count.

## Output

Structured JSON with phase timings, per-scenario results, and Confluence auto-publishing:

```json
{
  "replicas_requested": 1000,
  "pods_running": 1000,
  "pods_failed": 0,
  "injection_success_rate": 100.0,
  "time_to_all_ready_ms": 91400,
  "phase_timings": {
    "reset_ms": 2000,
    "warmup_ms": 210000,
    "execution_ms": 95000
  }
}
```

## Build

```bash
cargo build --release    # cargo
nix build                # hermetic nix build
```

Built with substrate's `rust-tool-release-flake.nix` — cross-platform binaries
for 4 targets (aarch64-darwin, x86_64-darwin, aarch64-linux, x86_64-linux).

## Signal Handler

Ctrl+C at any point triggers graceful cleanup:
1. Scale deployment to 0
2. Force-delete burst pods
3. Resume suspended kustomizations
4. Scale node group to 0

No orphaned pods or costly nodes left running.
