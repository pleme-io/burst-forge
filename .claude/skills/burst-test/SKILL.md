---
name: burst-test
description: Set up and run Akeyless injection burst tests on EKS. Use when configuring burst-forge, running scaling matrix tests, troubleshooting burst test failures, or preparing the cluster for scale testing.
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

## Running Burst Tests

### Full Scaling Matrix (recommended)
```bash
burst-forge matrix --kubeconfig /tmp/eks-scale-test.kubeconfig
```

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

## Scaling Matrix (historical data)

| Pods | Gateway | Webhook | Nodes (m5.xlarge) |
|------|---------|---------|-------------------|
| 50 | 1 | 1 | 1-2 |
| 150 | 1 | 2 | 3-4 |
| 300 | 2 | 3 | 6-7 |
| 500 | 3 | 4 | 10-11 |
| 750 | 5 | 5 | 14-15 |
| 1000 | 6 | 7 | 18-19 |

Formula: Gateway replicas = pods / 90 with 40% headroom.

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
