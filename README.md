# burst-forge

Kubernetes burst test orchestrator -- coordinated pod scaling with Akeyless
injection verification. Scales deployments from 0 to N pods, measures timing,
and reports injection success rates.

## What It Does

burst-forge automates Akeyless secret injection scale testing on Kubernetes.
Each burst test:

1. Resets a target deployment to 0 replicas
2. Scales to N replicas simultaneously
3. Polls pod status until all pods are Running (or timeout)
4. Counts pods with Akeyless sidecar injection (2+ containers = injected)
5. Reports timing (first ready, all ready), injection rate, and failure counts

The scaling matrix feature runs multiple scenarios back-to-back, adjusting
Akeyless gateway and webhook replica counts via HelmRelease patching to find
the right infrastructure sizing for a target pod count.

## Commands

| Command | Description |
|---------|-------------|
| `verify` | Check infrastructure readiness: nodes, Akeyless gateway/webhook, deployment, image cache |
| `wait` | Poll FluxCD kustomizations until all reach `Ready` status |
| `burst` | Scale 0 -> N, measure timing and injection rate. Supports `--replicas` and `--iterations` |
| `matrix` | Run all configured scaling scenarios. Patches HelmRelease replicas between scenarios |
| `reset` | Scale the target deployment back to 0 replicas |

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

# Image cache verification (optional)
cache_registry: "image-cache.image-cache.svc.cluster.local:5000"
required_images:
  - "akeyless/k8s-secrets-sidecar:0.35.1"
  - "akeyless/k8s-webhook-server:latest"
  - "library/nginx:1.27-alpine"

# FluxCD kustomizations to wait for (burst-forge wait)
flux_kustomizations:
  - infrastructure-image-cache
  - infrastructure-image-sync
  - infrastructure-akeyless
  - infrastructure-akeyless-injection
  - scale-test-workloads

# Akeyless infrastructure
akeyless_namespace: akeyless-system
gateway_label: "app.kubernetes.io/name=akeyless-api-gateway"
webhook_label: "app=akeyless-secrets-injection"
gateway_release: akeyless-gateway
webhook_release: akeyless-secrets-injection

# Scaling matrix scenarios
scenarios:
  - name: "50-pods"
    replicas: 50
    gateway_replicas: 1
    webhook_replicas: 1

  - name: "300-pods"
    replicas: 300
    gateway_replicas: 2
    webhook_replicas: 3

  - name: "1000-pods"
    replicas: 1000
    gateway_replicas: 6
    webhook_replicas: 7
```

See `examples/burst-forge.yaml` for a complete configuration with all scenario tiers.

## Prerequisites

- **EKS cluster** with FluxCD bootstrap (the scale-test cluster in `k8s/clusters/scale-test/`)
- **Akeyless gateway** and **secrets injection webhook** deployed via FluxCD HelmReleases
- **Image cache** (Zot registry + image-sync CronJob) to avoid Docker Hub rate limits
- **kubectl** on PATH with a valid kubeconfig pointing to the scale-test cluster
- **Target deployment** (e.g., `nginx-burst`) with Akeyless injection annotations

## Starting Conditions for Burst Tests

Before running a burst test, verify the cluster is ready:

```sh
# 1. Ensure all FluxCD kustomizations are reconciled
burst-forge wait

# 2. Verify infrastructure (nodes, gateway, webhook, image cache)
burst-forge verify

# 3. Check that deployment exists and starts at 0 replicas
kubectl -n scale-test get deployment nginx-burst
```

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

# Reset deployment to 0 replicas
burst-forge reset

# Use a specific kubeconfig
burst-forge --kubeconfig /path/to/kubeconfig verify
```

## Output

All commands output structured JSON. Burst results include:

```json
{
  "timestamp": "2026-03-30T...",
  "replicas_requested": 300,
  "pods_running": 300,
  "pods_failed": 0,
  "pods_pending": 0,
  "pods_with_sidecar": 281,
  "injection_success_rate": 93.7,
  "time_to_first_ready_ms": 2340,
  "time_to_all_ready_ms": 45200,
  "duration_ms": 50500,
  "nodes": 8,
  "iteration": 1
}
```

Matrix reports aggregate results across all scenarios for comparison.

## Build

Built with substrate's `rust-tool-release-flake.nix`:

```sh
nix build    # local binary
nix run      # run directly
```
