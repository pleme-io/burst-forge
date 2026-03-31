# Experiment Cycle

burst-forge experiment cycles are the systematic method for discovering and
eliminating bottlenecks in Kubernetes secret injection at scale.

## The Cycle

```
1. IDENTIFY   -- analyze previous run output for the limiting factor
2. HYPOTHESIZE -- propose ONE change and predict its effect
3. DOCUMENT   -- write experiment page on Confluence (before running)
4. APPLY      -- make the single change (k8s manifest, config, or infra)
5. RUN        -- burst-forge matrix --scenario "1000-pods"
6. MEASURE    -- read burst output: Running/Pending/Failed/Injected + timing
7. REPORT     -- update Confluence with actual results vs. prediction
8. ITERATE    -- back to step 1 with new data
```

### Rules

- **Change ONE variable per iteration.** If you change two things, you can't
  attribute the result to either one.
- **Document BEFORE running.** The hypothesis and expected outcome go on
  Confluence before the experiment fires. This prevents post-hoc rationalization.
- **Keep the full chain.** Every bottleneck discovered gets a number and stays
  in the chain table, even after it's fixed. The chain tells the story.

## What Each Phase Tells You

### Phase 1: RESET (should be < 5s)

If reset takes long, pods are stuck terminating. Check `terminationGracePeriodSeconds`
on the deployment and whether finalizers are blocking deletion.

### Phase 2: WARMUP (typically 2-6 min)

The warmup phase has 5 sub-steps, each revealing a different bottleneck class:

| Step | What it measures | Bottleneck if slow |
|------|------------------|--------------------|
| 2a. Nodes | EKS node group scale-up | EC2 launch time, ASG capacity |
| 2b. Images | DaemonSet image pre-pull | Image size, registry throughput |
| 2c. Gateway | GW deployment rollout | Readiness probe delay, IPAMD, scheduling |
| 2d. Webhook | WH deployment rollout | Same as gateway |
| 2e. Gates | Infrastructure verification | Any of the above not converging |

**Gate 3 failure (GW or WH not ready)** is the most common warmup failure.
Causes: nodeSelector mismatch, CPU exhaustion on target nodes, IPAMD IP
assignment failure. Always check pod events with `kubectl describe pod`.

### Phase 3: EXECUTION (the actual measurement)

The burst output columns tell you exactly where things stall:

| Column | Meaning | Healthy | Unhealthy |
|--------|---------|---------|-----------|
| Running | Pods with all containers started | Climbing to N | Stuck below N |
| Pending | Admitted but not scheduled/started | Draining to 0 | Stuck > 0 |
| Failed | Pods that crashed or were rejected | 0 | > 0 |
| Injected | Pods with injection env vars present | Climbing to N | Stuck below N |

**Reading the data:**

- **Injected climbs but Running stuck** -- scheduling bottleneck (CPU, IPAMD, node capacity)
- **Running climbs but Injected stuck** -- injection bottleneck (gateway QPS, webhook timeout)
- **Failed > 0** -- crash-on-error, image pull failures, or OOM
- **Pending stuck, Injected stuck** -- webhook timeout (pods never admitted)

## Pod Injection Lifecycle

Every tunable in the path from pod creation to Running with secrets:

```
API Server -- creates pod object
    |
    v
Mutating Webhook (Akeyless) -- injects init container + env vars
    | tunables: webhookTimeoutSeconds, WH replicas, WH CPU
    v
Scheduler -- assigns pod to node
    | tunables: nodeSelector, CPU requests, pods_per_node, node count
    v
Kubelet -- creates pod sandbox
    | tunables: VPC CNI prefix delegation, WARM_PREFIX_TARGET, IPAMD
    v
Init Container -- fetches secret from gateway
    | tunables: GW QPS (5/replica), GW replicas, CRASH_POD_ON_ERROR,
    |           AGENT_REQUESTS_CPU, AGENT_LIMITS_CPU
    v
Main Container -- starts with injected env vars
    | tunables: image pull (warmup DaemonSet), container startup time
    v
Pod Running -- burst-forge detects injection via env prefix
```

## Cluster Topology

Three node groups serve different roles. Infrastructure pods (GW, WH) MUST run
on workers with warm IPAMD. Burst pods run on burst nodes.

```
scale-test-system:   1x t3.medium  -- FluxCD, CoreDNS, image-cache, Zot
scale-test-workers:  4x t3.medium  -- GW (up to 11), WH (up to 7), warm IPAMD
scale-test-burst:    Nx m5.xlarge  -- burst pods, managed by burst-forge lifecycle
```

Workers use `nodeSelector` via HelmRelease `postRenderers` (Akeyless charts don't
support nodeSelector in values). burst-forge suspends HelmReleases during tests
to prevent FluxCD from reverting replica counts.

## Bottleneck Chain (living document)

| # | Bottleneck | Root Cause | Fix | Status |
|---|-----------|-----------|-----|--------|
| 1 | Webhook timeout 10s | K8s admission window too short | webhookTimeoutSeconds: 30 | FIXED |
| 2 | Gateway readiness 120s | Conservative probe | initialDelaySeconds: 30 | FIXED |
| 3 | FluxCD reverts scaling | GitOps reconciliation overwrites replicas | Suspend during burst | FIXED |
| 4 | CNI stall on new nodes | IPAMD warm-up race | hostNetwork on warmup DaemonSet | FIXED |
| 5 | Gateway QPS=5/replica | client-go default throttle | Scale replicas horizontally | ACCEPTED |
| 6 | VPC CNI IP exhaustion | Individual IP allocation mode | Prefix delegation | FIXED |
| 7 | IPAMD warm-up on burst nodes | New nodes need 2-3 min for IPs | Pin infra to workers | FIXED |
| 8 | Worker CPU for 11 GW | 2 t3.medium workers too small | Scale workers to 4 | FIXED |
| 9 | Init container crash | CRASH_POD_ON_ERROR=enable | Disable crash-on-error | FIXED |
| 10 | GW nodeSelector via values | Chart ignores nodeSelector in values | HelmRelease postRenderers | FIXED |
| 11 | Agent CPU request 250m | Init container dominates scheduling | Reduce to 25m/100m | TESTING |

## Cost Awareness

Burst nodes are m5.xlarge at ~$0.192/hr. 19 nodes = $3.65/hr = $87.60/day.
Always verify nodes are scaled to 0 after experiments:

```bash
burst-forge nodes status
# Or: aws eks describe-nodegroup --cluster-name scale-test \
#   --nodegroup-name scale-test-burst --query 'nodegroup.scalingConfig'
```

The 5 permanent nodes (1 system + 4 workers) cost ~$0.21/hr = $5/day.
