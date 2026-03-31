# Experiment Cycle

Systematic method for discovering and eliminating bottlenecks in Kubernetes
secret injection at scale.

## The Cycle

```
1. IDENTIFY    -- analyze previous run output for the limiting factor
2. HYPOTHESIZE -- propose ONE change and predict its effect
3. DOCUMENT    -- write experiment page on Confluence (before running)
4. APPLY       -- make the single change (k8s manifest, config, or infra)
5. RUN         -- burst-forge flow <name> [--scenario <tier>]
6. MEASURE     -- read burst output: Running/Pending/Failed/Injected + timing
7. REPORT      -- update Confluence with actual results vs. prediction
8. ITERATE     -- back to step 1 with new data
```

### Rules

- **Change ONE variable per iteration.** Two changes = ambiguous attribution.
- **Document BEFORE running.** Hypothesis and expected outcome on Confluence
  before the experiment fires. Prevents post-hoc rationalization.
- **Keep the full chain.** Every bottleneck gets a number and stays in the
  chain table, even after fixed. The chain tells the story.
- **Config is the source of truth.** Every experiment has a YAML config in
  `configs/` that captures ALL parameters. No ad-hoc CLI flags or env vars.

## Running Experiments

```bash
# Full matrix (all tiers 50-1000)
burst-forge flow cerebras-matrix

# Single scenario from a matrix
burst-forge flow cerebras-matrix --scenario cerebras-300

# Quick iteration on 1000-pod scenario
burst-forge flow single-1000
```

The `flow` subcommand discovers `configs/{name}.yaml` and handles the full
lifecycle: kustomization suspension, node scaling, IPAMD warmup, burst execution,
Confluence publishing, and teardown. Zero manual steps.

## What Each Phase Tells You

### Phase 1: RESET (should be < 5s)

If slow, pods are stuck terminating. Check `terminationGracePeriodSeconds`.

### Phase 2: WARMUP (typically 3-6 min)

| Step | What it measures | Bottleneck if slow |
|------|------------------|--------------------|
| 2a. Nodes | EKS node group scale-up | EC2 launch time, ASG capacity |
| 2b. Images | DaemonSet image pre-pull | Image size, registry throughput |
| 2b+. IPAMD | Secondary ENI attachment | Subnet capacity, WARM_PREFIX_TARGET |
| 2c. Gateway | GW deployment rollout | Readiness probe, CPU scheduling |
| 2d. Webhook | WH deployment rollout | Same as gateway |
| 2e. Gates | Infrastructure verification | Any of the above not converging |

### Phase 3: EXECUTION (the measurement)

| Column | Healthy | Unhealthy | Root cause |
|--------|---------|-----------|------------|
| Running climbs to N | OK | Stuck | Scheduling (CPU, IPAMD, node capacity) |
| Injected climbs to N | OK | Stuck | Injection (gateway QPS, webhook timeout) |
| Failed > 0 | - | Bad | crash-on-error, image pull, OOM |
| Pending stuck | - | Bad | Webhook timeout (pods never admitted) |

## Pod Injection Lifecycle

Every tunable from pod creation to Running with secrets:

```
API Server → Mutating Webhook → Scheduler → Kubelet → Init Container → Main Container → Running
               |                    |           |           |
               WH replicas       nodeSelector  IPAMD     GW QPS (5/replica)
               WH timeout        CPU requests  prefix    CRASH_POD_ON_ERROR
                                  maxPods      delegation AGENT_REQUESTS_CPU
```

## Bottleneck Chain (14 discovered)

| # | Bottleneck | Fix | Status |
|---|-----------|-----|--------|
| 1 | Webhook timeout 10s | 30s | FIXED |
| 2 | GW readiness 120s | 30s initialDelay | FIXED |
| 3 | FluxCD reverts scaling | Suspend HelmReleases + kustomizations | FIXED |
| 4 | CNI stall on new nodes | hostNetwork warmup DaemonSet | FIXED |
| 5 | GW QPS=5/replica | Scale replicas horizontally | ACCEPTED |
| 6 | VPC CNI IP mode | Prefix delegation | FIXED |
| 7 | IPAMD warmup on burst | Pin infra to workers + ipamd_warmup_secs | FIXED |
| 8 | Worker CPU for 11 GW | 4 workers | FIXED |
| 9 | Init crash-on-error | CRASH_POD_ON_ERROR=disable | FIXED |
| 10 | Chart nodeSelector | HelmRelease postRenderers | FIXED |
| 11 | Agent CPU 250m | 25m request / 100m limit | FIXED |
| 12 | FluxCD workload revert | suspend_kustomizations config | FIXED |
| 13 | /24 subnet exhaustion | /20 subnets + custom networking | FIXED |
| 14 | burst-forge premature exit | Fixed CAPACITY LIMIT detection | FIXED |

## Cluster Topology

```
scale-test-system:   1x t3.medium  -- FluxCD, CoreDNS, image-cache
scale-test-workers:  3-4x t3.medium -- GW + WH (warm IPAMD, nodeSelector via postRenderers)
scale-test-burst:    0-19x m5.xlarge -- burst pods (burst-forge lifecycle)
```

Infrastructure pods MUST run on workers (warm IPAMD). Burst pods on burst nodes.
VPC CNI custom networking: pod IPs from /20 subnets, node IPs on /24 subnets.

## Cost Awareness

Burst nodes: m5.xlarge ~$0.192/hr. 19 nodes = $3.65/hr = $87.60/day.
burst-forge scales to 0 on cleanup and Ctrl+C. Always verify after experiments:

```bash
burst-forge nodes status
```
