# Cerebras Optimization Experiment Runbook

## Pre-Flight Checklist

```bash
# 1. Verify cluster access
kubectl --kubeconfig ~/.kube/scale-test.yaml get nodes

# 2. Verify observability stack is running
kubectl -n observability get pods
# Expect: vector, shinryu-mcp, grafana, victoriametrics, loki all Running

# 3. Verify GW/WH metrics are being scraped
kubectl -n observability exec deploy/vector -- curl -s http://akeyless-gateway-akeyless-api-gateway.akeyless-system.svc:28888/metrics | head -5

# 4. Verify Shinryu has data
shinryu-mcp --analytics-path /tmp/test --sql "SELECT 1"

# 5. Open Grafana dashboard
# Navigate to: http://grafana.observability.svc:3000/d/burst-forge-experiments
```

## Phase Execution Order

Each phase builds on previous results. Run sequentially with analysis between phases.

### Phase 1: GW Memory Sweep (highest value — unblocks everything)

```bash
burst-forge flow phase1-gw-memory-sweep --output json
```

**Hypothesis:** 1Gi is the memory knee. 1536Mi/2Gi show diminishing returns.
**Decision:** If 768Mi ~ 1Gi: use 768Mi (cheaper). If WH=12 cliff disappears at 1Gi: WH contention is memory-caused.

**After Phase 1 — analyze via Shinryu:**
```bash
shinryu-mcp --experiment phase1-gw-memory-sweep-YYYYMMDDTHHMMZ --analysis predict
shinryu-mcp --experiment phase1-gw-memory-sweep-YYYYMMDDTHHMMZ --analysis bottleneck
```

### Phase 2: WH Contention Revalidation

```bash
burst-forge flow phase2-wh-contention-remap --output json
```

**Hypothesis:** With optimal GW memory, WH cliff at >=8 disappears.
**Decision:** If cliff gone → WH=5 universally optimal. If cliff persists → WH=3 for <=300, WH=5 for >=500.

### Phase 3: GW Replica Sweep

```bash
burst-forge flow phase3-gw-replica-sweep --output json
```

**Hypothesis:** More GW = proportionally faster (linear scaling).
**Decision:** If 20 GW < 10% gain over recommended: scaling saturates at recommended count.

### Phase 4: Network Analysis

```bash
burst-forge flow phase4-network-analysis --output json
```

**Hypothesis:** NetworkPolicy adds < 5% overhead.

### Phase 5: Node Topology

```bash
burst-forge flow phase5-node-topology --output json
```

### Phase 6: WH Memory Sweep

```bash
burst-forge flow phase6-wh-memory-sweep --output json
```

### Phase 7: GW Readiness

```bash
burst-forge flow phase7-gw-readiness --output json
```

### Phase 8: Combined Optimal (Final Validation)

```bash
burst-forge flow phase8-combined-optimal --output json
```

**This produces the final recommended configuration for Cerebras.**

## Quick Experiment Commands

```bash
# Run cerebras optimal config (validated: 300 pods in 37.4s)
burst-forge flow cerebras-optimal

# Run full matrix (50-1000 pods)
burst-forge flow cerebras-matrix

# Single scenario from a config
burst-forge flow phase1-gw-memory-sweep --scenario wh5-1gi

# Generate experiment plan from profile
burst-forge plan --profile cerebras --cluster scale-test

# Validate profile
burst-forge profile validate --profile cerebras
```

## Prediction Analysis

After any experiment with the updated burst-forge (now includes predictions):

```bash
# Check prediction accuracy via Shinryu MCP
shinryu-mcp --experiment <experiment-id> --analysis predict

# Raw SQL: prediction variance with memory correlation
shinryu-mcp --sql "$(cat queries/prediction-variance.sql | sed 's/{experiment_id}/<id>/')"

# Compare two experiments
shinryu-mcp --experiment <exp-a> --analysis compare --compare-with <exp-b>
```

## Scaling Formulas (Validated)

| Formula | Expression |
|---------|-----------|
| GW for sub-90s | `ceil(pods * secrets / (qps * 67))` |
| GW for sub-3min | `ceil(pods * secrets / (qps * 91))` |
| WH optimal (<=300) | 3 |
| WH optimal (>=500) | 5 |
| GW memory min | 768Mi (WH<=5), 1Gi (WH>5) |
| Theoretical floor | `(pods * secrets) / (gw * qps)` seconds |

## Current Optimal Config (Cerebras ASM-17583)

| Parameter | Value |
|-----------|-------|
| GW replicas | 5 (for 300 pods) |
| WH replicas | 3 |
| GW memory | 1Gi |
| GW CPU | 500m |
| WH timeout | 30s |
| Agent CPU | 25m/100m |
| Failure policy | Fail |
| QPS per GW | 5 |
| Secrets per pod | 2 |
| **Result** | **300 pods in 37.4s, 100% injection** |
