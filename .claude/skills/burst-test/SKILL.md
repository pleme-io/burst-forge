---
name: burst-test
description: Run and configure Kubernetes secret injection burst tests. Use when running scaling matrix tests, configuring burst-forge flows, troubleshooting burst failures, analyzing bottlenecks, or preparing the cluster for scale testing.
---

# Burst Testing with burst-forge

## Quick Start

```bash
# Full Cerebras battery (50-1000 pods, 2 secrets, all tiers)
burst-forge flow cerebras-matrix

# Single scenario from a matrix
burst-forge flow cerebras-matrix --scenario cerebras-300

# Quick 1000-pod iteration
burst-forge flow single-1000

# List available flows
ls configs/*.yaml
```

The `flow` subcommand discovers `configs/{name}.yaml` and handles the full
lifecycle: kustomization suspension, node scaling, IPAMD warmup, burst execution,
inter-scenario drain, Confluence publishing, and teardown. Zero manual steps.

## Prerequisites

- EKS cluster with FluxCD bootstrapped (pleme-io/k8s repo)
- Akeyless gateway + injection webhook deployed (via HelmRelease)
- Images in ECR (not Docker Hub)
- AWS credentials: `aws sso login --profile akeyless-development`
- `cargo build --release` (binary at `target/release/burst-forge`)

## Config Structure

Each flow config is a self-contained YAML:

```yaml
kubeconfig: "~/.kube/scale-test.yaml"
namespace: scale-test
deployment: nginx-burst
ipamd_warmup_secs: 150
suspend_kustomizations:
  - scale-test-workloads

node_group:
  cluster_name: scale-test
  nodegroup_name: scale-test-burst
  ...

scenarios:
  - name: "1000-pods"
    replicas: 1000
    gateway_replicas: 11
    webhook_replicas: 7
  ...
```

**Private configs** (Confluence keys, access IDs) live in pleme-io/k8s (private),
not in the public burst-forge repo.

## 3-Phase Lifecycle

```
Phase 1: RESET   -> Gate: Zero State
Phase 2: WARMUP  -> 2a: Nodes -> 2b: Images -> 2b+: IPAMD -> 2c: GW -> 2d: WH -> 2e: Gates
Phase 3: EXECUTE -> burst 0->N, poll Running/Pending/Failed/Injected, measure timing
```

Each phase is independently timed. The `flow` command also handles:
- Kustomization suspend BEFORE Phase 2 (prevents FluxCD replica revert)
- Kustomization resume AFTER all scenarios (cleanup)
- Node teardown AFTER all scenarios
- Ctrl+C cleanup (resume kustomizations + scale nodes to 0)

## Reading Burst Output

| Pattern | Meaning | Fix |
|---------|---------|-----|
| Injected=N, Running stuck | Scheduling bottleneck | CPU, IPAMD, node capacity |
| Running climbs, Injected stuck | Injection bottleneck | GW QPS, WH timeout |
| Failed > 0 | Crash-on-error or OOM | CRASH_POD_ON_ERROR, agent resources |
| Pending stuck at 0 | Webhook timeout | webhookTimeoutSeconds, WH replicas |
| Running=N, then drops to 0 | FluxCD revert | suspend_kustomizations config |

## Key Tunables

| Tunable | Config field | Default | Impact |
|---------|-------------|---------|--------|
| GW replicas | `scenarios[].gateway_replicas` | per scenario | 5 req/s per replica (QPS locked) |
| WH replicas | `scenarios[].webhook_replicas` | per scenario | ~33 admissions/sec per replica |
| WH timeout | k8s HelmRelease | 30s | Admission window per pod |
| IPAMD warmup | `ipamd_warmup_secs` | 0 | Custom networking ENI setup |
| Agent CPU | HelmRelease env | 25m | Pod scheduling footprint |
| CRASH_POD_ON_ERROR | HelmRelease env | disable | Init container retry vs crash |
| Burst nodes | `node_group.max_nodes` | 20 | Ceiling for node scaling |
| Pod subnets | ENIConfig CRD | /20 per AZ | 4094 IPs vs /24 = 254 IPs |

## Experiment Methodology

See `docs/experiment-cycle.md` for the full cycle. Key principles:
- Change ONE variable per iteration
- Document hypothesis on Confluence BEFORE running
- Config YAML captures ALL parameters (no ad-hoc flags)
- Every bottleneck gets a number in the chain table

## 14 Bottlenecks Discovered

| # | Bottleneck | Fix |
|---|-----------|-----|
| 1 | Webhook timeout 10s | 30s |
| 2 | GW readiness 120s | 30s initialDelay |
| 3 | FluxCD reverts | Suspend HelmReleases + kustomizations |
| 4 | CNI stall | hostNetwork warmup |
| 5 | GW QPS=5 | Scale replicas |
| 6 | VPC CNI mode | Prefix delegation |
| 7 | IPAMD warmup | Pin infra to workers + ipamd_warmup_secs |
| 8 | Worker CPU | 4 workers |
| 9 | Init crash | CRASH_POD_ON_ERROR=disable |
| 10 | Chart nodeSelector | postRenderers |
| 11 | Agent CPU 250m | 25m/100m |
| 12 | FluxCD workload revert | suspend_kustomizations |
| 13 | /24 subnet exhaustion | /20 subnets + custom networking |
| 14 | Premature exit | Fixed CAPACITY LIMIT detection |

## Cluster Topology

```
scale-test-system:   1x t3.medium   (FluxCD, CoreDNS, image-cache)
scale-test-workers:  3-4x t3.medium (GW+WH, warm IPAMD, postRenderers nodeSelector)
scale-test-burst:    0-19x m5.xlarge (burst pods, burst-forge lifecycle)
```

IaC: Pangea EksScaleTest (burst node group, /20 subnets, worker scaling).
GitOps: pleme-io/k8s (HelmReleases, ENIConfig, VPC CNI config).

## Troubleshooting

### Gate 3 fails (GW not ready)
Check worker node capacity: `kubectl describe node -l eks.amazonaws.com/nodegroup=scale-test-workers`

### 0 Running despite 1000 Injected
IPAMD issue. Increase `ipamd_warmup_secs` or check VPC CNI custom networking.

### Pods disappear mid-burst
FluxCD kustomization revert. Add the workload kustomization to `suspend_kustomizations`.

### Nodes not scaling
Check AWS SSO: `aws sts get-caller-identity --profile akeyless-development`
