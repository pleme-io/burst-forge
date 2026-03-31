---
name: burst-test
description: Set up and run secrets injection burst tests on Kubernetes. Use when configuring burst-forge, running scaling matrix tests, troubleshooting burst test failures, or preparing the cluster for scale testing. Works with any injection system (Akeyless, Vault, etc.) via config.
---

# Burst Testing with burst-forge

## Prerequisites

- EKS cluster with FluxCD bootstrapped (pleme-io/k8s repo)
- Akeyless gateway + injection webhook deployed (via HelmRelease)
- Images in ECR (not Docker Hub — rate limits)
- burst-forge config at `~/.config/burst-forge/burst-forge.yaml`
- AWS credentials (`aws sso login --profile akeyless-development`)
- kubeconfig: `AWS_PROFILE=akeyless-development aws eks update-kubeconfig --name scale-test --region us-east-1 --kubeconfig /tmp/eks-scale-test.kubeconfig`

## Starting Conditions Checklist

Before running burst tests, verify:

1. All FluxCD kustomizations are True:
   ```
   KUBECONFIG=/tmp/eks-scale-test.kubeconfig kubectl get kustomizations -n flux-system
   ```

2. Akeyless gateway and injection webhook are Running:
   ```
   KUBECONFIG=/tmp/eks-scale-test.kubeconfig kubectl get pods -n akeyless-system
   ```

3. nginx-burst deployment exists at 0 replicas:
   ```
   KUBECONFIG=/tmp/eks-scale-test.kubeconfig kubectl get deploy nginx-burst -n scale-test
   ```

4. System nodes (1x t3.medium) and worker nodes (2x t3.medium) are Ready
5. Burst nodes at 0 (burst-forge will scale them up)

## 3-Phase Lifecycle Architecture

Every burst (single or matrix) follows a strict 3-phase lifecycle with 5 explicit gates:

```
Phase 1: RESET      -> Gate: Zero State (deployment=0, no orphan pods)
Phase 2: WARMUP     -> Gate: Node Ready (all burst nodes Ready)
                    -> Gate: Image Cache (DaemonSet pods Running on all nodes)
                    -> Gate: Infra Ready (gateway + webhook rollout complete)
                    -> Gate: Warmup Complete (all 3 above passed)
Phase 3: EXECUTION  -> Scale 0->N, poll injection, measure timing
```

Each gate returns a `GateResult` with expected/actual diagnostics. If any gate
fails, the burst is aborted with a clear error (no partial/ambiguous results).

Phase timing is independently measured: `reset_duration`, `warmup_duration`,
`execution_duration` are all in the JSON output.

## 9 Bottlenecks Discovered

Testing at scale revealed 9 distinct bottlenecks. The tunables table below
maps each bottleneck to the configuration knob that controls it.

| # | Bottleneck | Symptom | Root Cause | Resolution |
|---|-----------|---------|------------|------------|
| 1 | Docker Hub rate limits | `ImagePullBackOff` at >100 pods | 100 pulls/6h anonymous | ECR mirror + Zot in-cluster cache |
| 2 | Cold node image pull | 30-60s pod startup on new nodes | Images not pre-pulled | `image-warmup` DaemonSet + burst-forge waits for it |
| 3 | Webhook timeout | Injection failures under load | Default 10s too short | `timeoutSeconds: 30` via postRenderers patch |
| 4 | Gateway saturation | Slow/failed token issuance | Too few gateway replicas | Scale GW replicas per formula: `pods / 90` |
| 5 | Webhook saturation | Pods stuck in `Init` | Too few webhook replicas | Scale webhook replicas: `pods / 75` |
| 6 | maxSurge throttling | Pods created in waves, not simultaneously | Default 25% maxSurge | burst-forge patches `maxSurge` to replica count |
| 7 | Namespace selector missing | Webhook deadlock (injects into own pods) | No namespace filtering | `namespaceSelector: akeyless-injection: enabled` |
| 8 | HelmRelease drift | Gateway/webhook replicas reset after reconcile | FluxCD overrides manual patches | burst-forge patches HelmRelease `.spec.values` |
| 9 | Node capacity | `CAPACITY LIMIT` with fewer pods than requested | Not enough burst nodes | burst-forge calculates nodes: `ceil(replicas / pods_per_node)` |

## Tunables Table

| Tunable | Config Key | Default | Effect |
|---------|-----------|---------|--------|
| Gateway replicas | `scenarios[].gateway_replicas` | per scenario | More replicas = more concurrent token issuance |
| Webhook replicas | `scenarios[].webhook_replicas` | per scenario | More replicas = more concurrent injection |
| Timeout | FluxCD postRenderers | 30s | Longer timeout = fewer injection failures under load |
| Burst nodes | `node_group.max_nodes` | 20 | Hard ceiling for node scaling |
| Pods per node | `node_group.pods_per_node` | 58 | Used to calculate required node count |
| Injection mode | `injection_mode` | `env` | `env` (AKEYLESS_TOKEN) or `sidecar` (extra container) |
| maxSurge | auto-patched | replicas count | Ensures all pods created simultaneously |
| Image warmup | DaemonSet | always | Pre-pulls images to every node before burst |

## Experiment Methodology

Each matrix run is a controlled experiment:

1. **Reset** -- `reset-all` brings the entire environment to a known zero state
2. **Warmup** -- Nodes scaled, images pre-pulled, gateway/webhook replicas set
3. **Execute** -- Deployment scaled 0->N atomically (maxSurge = N)
4. **Measure** -- Poll until all pods Running or timeout, record injection rate + timing
5. **Collect** -- JSON results with per-pod injection status, gate diagnostics, phase timing
6. **Repeat** -- Next scenario (descending: 1000 -> 500 -> 50 for maximum initial pressure)

Results are deterministic because every variable is controlled: same node count,
same image state, same gateway/webhook replicas, same maxSurge.

## Running Burst Tests

Config lives in the project repo, symlinked for shikumi discovery:
```
pleme-io/akeyless-k8s/test/scale-test/burst-forge.yaml  (source of truth)
~/.config/burst-forge/burst-forge.yaml                    (symlink)
```

### Full Scaling Matrix (recommended)
```bash
# From akeyless-k8s project directory:
burst-forge matrix --config test/scale-test/burst-forge.yaml --kubeconfig /tmp/eks-scale-test.kubeconfig

# Or via shikumi discovery (if symlinked):
burst-forge matrix --kubeconfig /tmp/eks-scale-test.kubeconfig
```

### ASM-17583 Scalability Validation
The scenarios match the ASM-17540 table (Cerebras). Run largest first, descending — maximum pressure on gateway/webhook first, then validate lower tiers.

### Signal Handling
Ctrl+C triggers graceful cleanup: deployment scaled to 0, node group scaled to 0. No orphaned resources.

This automatically:
1. Calculates nodes needed from largest scenario
2. Scales burst node group up
3. Waits for nodes Ready + image warmup
4. Runs each scenario (scales gateway/webhook replicas)
5. Collects JSON results
6. Scales burst nodes back to 0

### Single Burst
```bash
burst-forge burst --replicas 50 --kubeconfig /tmp/eks-scale-test.kubeconfig
```

### Verify Infrastructure
```bash
burst-forge verify --kubeconfig /tmp/eks-scale-test.kubeconfig
```

### Wait for FluxCD
```bash
burst-forge wait --kubeconfig /tmp/eks-scale-test.kubeconfig
```

### Node Management
```bash
burst-forge nodes up --count 18 --kubeconfig /tmp/eks-scale-test.kubeconfig
burst-forge nodes status --kubeconfig /tmp/eks-scale-test.kubeconfig
burst-forge nodes down --kubeconfig /tmp/eks-scale-test.kubeconfig
```

### Reset-All Command

Full environment reset to pristine state:

```bash
burst-forge reset-all --kubeconfig /tmp/eks-scale-test.kubeconfig
```

This command:
1. Scales `nginx-burst` deployment to 0 replicas
2. Force-deletes any orphan pods in the namespace
3. Resets gateway HelmRelease replicas to 1
4. Resets webhook HelmRelease replicas to 1
5. Scales burst node group to 0

Use `reset-all` before starting a new matrix run, after a failed run, or when
the environment is in an unknown state. The regular `reset` command only scales
the deployment to 0 -- `reset-all` is the full teardown.

## Config Format

Located at `~/.config/burst-forge/burst-forge.yaml` (shikumi discovery):

```yaml
namespace: scale-test
deployment: nginx-burst
timeout_secs: 600

# Injection detection: "env" (AKEYLESS_TOKEN env var) or "sidecar" (2+ containers)
injection_mode: env

# FluxCD dependencies (waited in order)
flux_kustomizations:
  - infrastructure-image-cache
  - infrastructure-akeyless
  - infrastructure-akeyless-injection
  - scale-test-workloads

# EKS node group (burst-forge manages lifecycle)
node_group:
  cluster_name: scale-test
  nodegroup_name: scale-test-burst
  region: us-east-1
  aws_profile: akeyless-development
  pods_per_node: 58
  max_nodes: 20

# Scaling matrix
scenarios:
  - name: "1000-pods"
    replicas: 1000
    gateway_replicas: 6
    webhook_replicas: 7
  - name: "500-pods"
    replicas: 500
    gateway_replicas: 3
    webhook_replicas: 4
  - name: "50-pods"
    replicas: 50
    gateway_replicas: 1
    webhook_replicas: 1
```

## Node Architecture

| Node Group | Type | Count | Purpose | Managed by |
|------------|------|-------|---------|------------|
| scale-test-system | t3.medium | 1 | Zot, FluxCD, image-sync | Permanent |
| scale-test-workers | t3.medium | 2 | Akeyless gateway + webhook | Permanent |
| scale-test-burst | m5.xlarge | 0-20 | nginx-burst pods only | burst-forge |

## Injection Detection

Two modes based on how Akeyless injects secrets:

- **`env` mode** (default): Webhook injects `AKEYLESS_TOKEN` env var into existing containers. No extra sidecar. Detected by checking env vars.
- **`sidecar` mode**: Webhook adds a sidecar container. Detected by container count >= 2.

Set `injection_mode` in config or `--injection-mode` on CLI.

## Scaling Matrix (ASM-17540)

From the Jira ticket — Cerebras scalability validation:

| Range | GW | Injectors | Webhook | Status |
|-------|-----|-----------|---------|--------|
| ≤50 | 1 | 1 | 1 | Not yet tested |
| 51-150 | 1 | 2 | 2 | Partial — failed at 2 GW + 2 injector |
| 151-300 | 2 | 4 | 3-4 | 4 GW resolved 150-pod burst |
| 301-500 | 3 | 5-6 | 4 | 6 GW + 4 injector handled 500 |
| 501-750 | 4 | 7 | 5 | Not tested |
| 751-1000 | 5-6 | 9 | 6-7 | Not tested |

Scenarios run largest first (1000 → 50) for maximum pressure first.

### maxSurge
burst-forge patches `maxSurge` to match the scenario's replica count before each burst, ensuring all pods are created simultaneously for maximum concurrent pressure on the gateway/webhook.

## Confluence Reporting

Matrix results are automatically published to Confluence as formatted XHTML tables:

```bash
# Publish results from the last matrix run
burst-forge report --kubeconfig /tmp/eks-scale-test.kubeconfig

# Or publish a specific JSON file
burst-forge report --input results.json
```

The report includes:
- Scenario results table (replicas, gateway/webhook replicas, injection rate, timing)
- Gate diagnostics for each phase
- Per-phase timing breakdown (reset, warmup, execution)
- Node scaling details (requested, actual, time to Ready)
- Bottleneck analysis when injection rate < 100%

Config for Confluence publishing is in the burst-forge YAML:
```yaml
confluence:
  base_url: "https://your-instance.atlassian.net"
  space_key: "ENG"
  parent_page_id: "123456"
```

Credentials use `CONFLUENCE_USER` and `CONFLUENCE_TOKEN` environment variables.

## Nix Integration

burst-forge is built with substrate's `rust-tool-release` pattern:

```bash
# Run locally via Nix
nix run github:pleme-io/burst-forge -- matrix --kubeconfig /tmp/eks-scale-test.kubeconfig

# Or from the repo
cd ~/code/github/pleme-io/burst-forge
nix run .#default -- verify --kubeconfig /tmp/eks-scale-test.kubeconfig
```

Available `nix run` apps:
- `.#default` — burst-forge binary
- `.#release` — GitHub release workflow
- `.#regenerate-cargo-nix` — regenerate Cargo.nix

## Fleet Orchestration

burst-forge integrates with FluxCD as the fleet coordination layer. Complex workflows combine burst-forge commands with fleet state:

### Full Zero-to-Results Workflow
```bash
# 1. Ensure FluxCD chain is healthy
burst-forge wait --kubeconfig /tmp/eks-scale-test.kubeconfig --timeout 300

# 2. Verify infrastructure (cache, gateway, webhook)
burst-forge verify --kubeconfig /tmp/eks-scale-test.kubeconfig

# 3. Scale nodes, run matrix, teardown (all-in-one)
burst-forge matrix --kubeconfig /tmp/eks-scale-test.kubeconfig
```

### Manual Step-by-Step (for debugging)
```bash
# Scale nodes manually
burst-forge nodes up --count 18 --kubeconfig /tmp/eks-scale-test.kubeconfig

# Wait for fleet dependencies
burst-forge wait --kubeconfig /tmp/eks-scale-test.kubeconfig

# Verify everything is ready
burst-forge verify --kubeconfig /tmp/eks-scale-test.kubeconfig

# Run a single burst
burst-forge burst --replicas 50 --kubeconfig /tmp/eks-scale-test.kubeconfig

# Check results, adjust, repeat
burst-forge burst --replicas 300 --kubeconfig /tmp/eks-scale-test.kubeconfig

# Full reset and teardown
burst-forge reset-all --kubeconfig /tmp/eks-scale-test.kubeconfig
burst-forge nodes down --kubeconfig /tmp/eks-scale-test.kubeconfig
```

### Pipeline Integration
```bash
# CI/CD: single command, zero manual steps
# Config drives everything via shikumi YAML
KUBECONFIG=/tmp/eks.kubeconfig burst-forge matrix 2>&1 | tee burst-results.json
```

The `matrix` command is the top-level orchestrator that replaces manual fleet coordination:
1. Reads scenarios from YAML config
2. Scales EKS node group (tagged `burst-forge=true`)
3. Waits for FluxCD kustomizations in dependency order
4. Validates image cache populated
5. For each scenario: patches HelmRelease replicas → verifies → bursts → collects
6. Outputs full JSON report
7. Scales nodes back to 0

## Critical: Warmup and Cleanup

### Pre-Heating (MUST complete before burst)
Node warmup is critical — cold nodes cause ImagePull delays that skew results:
1. **Node scaling**: burst-forge scales the node group and waits for ALL nodes Ready
2. **Image warmup**: The `image-warmup` DaemonSet pre-pulls nginx + sidecar images to every node. burst-forge waits for the DaemonSet to report all pods Running before proceeding
3. **FluxCD health**: All kustomizations must be True — a partially reconciled cluster produces unreliable results
4. **Gateway/webhook rollout**: After patching HelmRelease replicas, burst-forge waits for the deployment rollout to complete (all new pods Ready)

If ANY warmup step is skipped, the burst results are invalid.

### Cleanup/Teardown (MUST run — cost reasons)
burst nodes are m5.xlarge at ~$0.192/hr each. 18 nodes = $3.46/hr = $83/day:
- `matrix` command automatically scales burst nodes to 0 after ALL scenarios complete
- If burst-forge is interrupted (Ctrl+C, crash, SSO timeout), manually teardown:
  ```bash
  burst-forge nodes down --kubeconfig /tmp/eks-scale-test.kubeconfig
  # Or directly:
  AWS_PROFILE=akeyless-development aws eks update-nodegroup-config \
    --cluster-name scale-test --nodegroup-name scale-test-burst \
    --scaling-config minSize=0,maxSize=20,desiredSize=0 --region us-east-1
  ```
- Always verify nodes are down after testing:
  ```bash
  burst-forge nodes status --kubeconfig /tmp/eks-scale-test.kubeconfig
  ```
- The 3 permanent nodes (system + workers) stay up at ~$0.13/hr total — acceptable for the test environment

## Troubleshooting

### Docker Hub Rate Limits
All images should be in ECR (376129857990.dkr.ecr.us-east-1.amazonaws.com).
To add a new image:
```bash
skopeo copy docker://docker.io/image:tag docker://376129857990.dkr.ecr.us-east-1.amazonaws.com/image:tag
```

### Stuck HelmRelease
```bash
KUBECONFIG=/tmp/eks-scale-test.kubeconfig kubectl delete helmrelease <name> -n akeyless-system
# Flux will recreate from Git
```

### Nodes Not Scaling
Check EKS node group max:
```bash
AWS_PROFILE=akeyless-development aws eks describe-nodegroup --cluster-name scale-test --nodegroup-name scale-test-burst --region us-east-1 --query 'nodegroup.scalingConfig'
```

### Injection Shows 0%
Check `injection_mode` in config. If using `akeyless/secret-output: "env"`, set `injection_mode: env`.
