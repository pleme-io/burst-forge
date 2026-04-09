# Session 2026-04-09 — Probe Optimization + Single-Wave Validation

## Summary

4 experiments at 10000 pods validated the optimal GW probe and scaling config.
GW scaling dropped from 30 min to 6 min (80% reduction). Total experiment
time from 50 min to 23.5 min (53% reduction).

## Experiment Results

| ID | Config | GW Scaling | Total Warmup | Peak Running | Failed |
|----|--------|-----------|-------------|-------------|--------|
| A+B+C | Probes + batch 10 + stabilize 30s | **6 min** | **23.5 min** | 9981 | 0 |
| D | Single-wave (all 30 at once) | 12.8 min | 20.6 min | 9980 | 0 |

### Experiment A: Probe Optimization (VALIDATED)

**Before:** `readinessProbe.initialDelaySeconds: 60, periodSeconds: 10`
**After:** `startupProbe: initialDelaySeconds: 15, periodSeconds: 5, failureThreshold: 12`

GW Loki profiling showed actual startup time is 19-23s (container start to
/health returning 200). The chart default `initialDelaySeconds: 60` wasted
40s per pod. At 30 pods across 6 waves, that's ~25 min of pure wait time.

### Experiment B: Larger GW Batch (VALIDATED)

**Before:** `gateway_batch_size: 5` (6 waves for 30 pods)
**After:** `gateway_batch_size: 10` (3 waves for 30 pods)

With optimized probes, each wave completes without timeout. Fewer waves =
faster total scaling.

### Experiment C: Reduced Stabilization (VALIDATED)

**Before:** `post_scale_stabilize_secs: 180`
**After:** `post_scale_stabilize_secs: 30`

No webhook regression. The optimized probes ensure pods are truly ready when
marked Ready, so less post-scale wait is needed.

### Experiment D: Single-Wave Scaling (VIABLE BUT SLOWER)

**Config:** `gateway_batch_size: 0` (all 30 at once, no batching)

Result: All 30 GW pods started simultaneously. 27/30 passed startup probe on
first attempt. 3/30 needed a restart (startup probe exhaustion at 60s). The
restarts added ~6 min overhead, making total GW scaling 12.8 min vs 6 min batched.

**Why batched beats single-wave:**
- 30 simultaneous JVM starts compete for CPU on 8 gateway nodes
- ClusterCache leader takes ~20s for full-sync; 29 followers queue for Redis
- 3 slowest pods exceed 60s startup window → restart → retry
- Batched waves give each cohort dedicated CPU + Redis bandwidth

## GW Cold-Start Profile (from Loki)

| Phase | Duration | Detail |
|-------|----------|--------|
| Container start → curl_proxy spawn | 18-22s | Go init, HSM, TLS, config, Redis |
| curl_proxy spawn → RUNNING | 1-2s | Config read, cache init |
| RUNNING → services started | <0.5s | API proxy, config handler |
| **Total to serving** | **19-23s** | Pod functional, /health returns 200 |
| Services → leadership | 0-2700s | Background, does NOT block serving |

## Cluster Stability Fixes (also this session)

| Fix | File | Root Cause |
|-----|------|------------|
| vector-logs CrashLoop | `vector/configmap.yaml` | VRL `??` on infallible expressions |
| node-exporter CrashLoop | `node-exporter/daemonset.yaml` | Missing amd64 nodeAffinity |
| EBS CSI CrashLoop | Runtime (Cilium restart) | Stale routing on long-lived node |
| Stuck namespace | `pangea-self-manage` | Orphaned finalizers on CRDs |
| 5 broken kustomizations | `flux-kustomizations/*.yaml` | Suspended: missing deps/CRDs |

## Next Experiments

| ID | What | Status |
|----|------|--------|
| E | Customer simulation (8 warm GW baseline) | Pending |
| F | Increase failureThreshold to 24 + retry single-wave | Pending |
| G | Investigate 20 persistent Pending pods (IPAMD) | Pending |
| H | Cost optimization: reduce IPAMD warmup from 300s | Pending |

## Cost

Each 10000-pod experiment costs ~$5 (was ~$8 before probe optimization).
Baseline cluster cost: ~$6.14/day (5 nodes).
