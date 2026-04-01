# Session Handoff — 2026-04-01

## What This Project Is

burst-forge is a Rust CLI for Kubernetes secret injection scalability testing.
Built for ASM-17583 (Cerebras customer needs ~300 concurrent pod injections via Akeyless).

## What Was Accomplished

### Deliverable: Complete
- 40+ experiments across 14 bottlenecks discovered and resolved
- Customer-facing guidance published: [What to Change and What to Expect](https://akeyless.atlassian.net/wiki/spaces/~7120203936f1d3939b4810895c20eb2bc58ae4/pages/3975905326)
- **Authoritative number: 300/300 pods in 37.4s, 100% injection** (10 GW, 3 WH, 2 secrets, 2 init containers)
- All data on Confluence under parent page ID 3973120030

### Key Findings
- Gateway at QPS=5 is the permanent throughput ceiling
- GW formula: `ceil(pods / 67)` for sub-90s at QPS=5
- WH: keep at 3 on shared nodes (more WH = slower due to CPU limit overcommitment via CFS throttling)
- Agent CPU: 25m request / 100m limit (default 250m blocks scheduling)
- CRASH_POD_ON_ERROR: disable (necessary for burst workloads)
- webhookTimeoutSeconds: 30 (10s rejects 92% of pods)
- Multi-secret: 10 secrets adds only 16% (near-linear)
- Jobs: confirmed identical to Deployments for injection
- Init container duration: no impact (0-10s sleep had no monotonic effect)

### burst-forge Features Built
- `flow` subcommand — declarative YAML configs, zero manual steps
- Per-scenario pod spec patches (init_sleep, memory, secrets)
- Per-scenario infrastructure patches (WH/GW CPU request/limit)
- Job workload support (WorkloadKind::Job)
- Worker node group auto-scaling + verified teardown
- IPAMD warmup + kustomization suspension
- Confluence auto-publishing

## What's In Progress

### WH Limit Sweep (INCONCLUSIVE — needs re-run)
`burst-forge flow investigate-wh-limit-sweep` — ran 5 scenarios but max_nodes=10
capped all at 550/1000 pods. At 550 pods the system is node-bound, not gateway-bound.
All 5 scenarios produced identical throughput (0.904-0.911 pods/s) — the test didn't
reach the gateway bottleneck point.

**Fix applied:** max_nodes changed to 20. Needs re-run to get meaningful data.
Published (inconclusive): https://akeyless.atlassian.net/wiki/spaces/~7120203936f1d3939b4810895c20eb2bc58ae4/pages/3978395670

### GW CPU Test (running now)
`burst-forge flow investigate-gw-cpu` — testing GW at 500m vs 1000m vs no limit.
max_nodes fixed to 20.

### Pangea State Alignment (analyzed — needs 30-min import session)
The Pangea code has been updated with burst node group + /20 subnets but NOT applied.
Ad-hoc AWS resources exist. **DO NOT run `pangea apply`** — it will try to create
resources that already exist and fail.

**Safe path:** terraform import the 5 ad-hoc resources into Pangea state:
```bash
terraform import aws_eks_node_group.scale-test-burst scale-test:scale-test-burst
terraform import aws_subnet.scale-test-pods-0 subnet-0a9f66f70b24b1fed
terraform import aws_subnet.scale-test-pods-1 subnet-076e7e717c6390e53
terraform import aws_route_table_association.scale-test-pods-0 <rtbassoc-id>
terraform import aws_route_table_association.scale-test-pods-1 <rtbassoc-id>
```
After import: `pangea plan` should show no changes.
State bucket: s3://pleme-dev-terraform-state/pangea/eks-scale-test

## Cluster State

- **burst nodes:** scaling to 10 (experiment running)
- **workers:** 3 × t3.medium
- **system:** 1 × t3.medium
- **Idle cost:** ~$4/day when burst=0

## Key Files

### burst-forge (~/code/github/pleme-io/burst-forge)
- `src/config.rs` — Config + Scenario structs with all fields
- `src/burst.rs` — Deployment burst + apply_scenario_patches + apply_infrastructure_patches
- `src/job.rs` — Job workload support
- `src/phases.rs` — 3-phase lifecycle (RESET → WARMUP → EXECUTION)
- `src/matrix.rs` — scenario orchestration with guaranteed cleanup
- `src/nodes.rs` — EKS node group management + nodegroup status polling
- `configs/` — all experiment configs (gap-*, investigate-*, sweep-*, cerebras-*)

### k8s repo (~/code/github/pleme-io/k8s)
- `clusters/scale-test/infrastructure/akeyless/` — GW HelmRelease + postRenderers
- `clusters/scale-test/infrastructure/akeyless-injection/` — WH HelmRelease + postRenderers
- `clusters/scale-test/infrastructure/vpc-cni/` — ENIConfig + DaemonSet env patch
- `clusters/scale-test/workloads/nginx-burst/` — burst deployment (2 init, 2 secrets)

### Pangea (~/code/github/pleme-io/pangea-architectures)
- `lib/pangea/architectures/eks_scale_test.rb` — architecture with burst group + /20 subnets
- `workspaces/eks-scale-test/` — deployment template
- `workspaces/akeyless-dev-config/` — Akeyless auth + secrets (hello1-10)
- `spec/architectures/eks_scale_test_spec.rb` — 29 passing RSpec tests

## Confluence Pages (all under parent 3973120030)
- [Customer Guidance (v6)](https://akeyless.atlassian.net/wiki/spaces/~7120203936f1d3939b4810895c20eb2bc58ae4/pages/3975905326)
- [Full Engineering Report](https://akeyless.atlassian.net/wiki/spaces/~7120203936f1d3939b4810895c20eb2bc58ae4/pages/3976003605)
- [Gap Closure Results](https://akeyless.atlassian.net/wiki/spaces/~7120203936f1d3939b4810895c20eb2bc58ae4/pages/3979182081)
- [Webhook Investigation](https://akeyless.atlassian.net/wiki/spaces/~7120203936f1d3939b4810895c20eb2bc58ae4/pages/3978788886)
- [Test Plan & Hypotheses](https://akeyless.atlassian.net/wiki/spaces/~7120203936f1d3939b4810895c20eb2bc58ae4/pages/3978756105)

## Memory Files
- `~/.claude/projects/-Users-luis-d-code-github/memory/exp4_bottleneck_chain.md`
- `~/.claude/projects/-Users-luis-d-code-github/memory/cerebras_customer_env.md`
- `~/.claude/projects/-Users-luis-d-code-github/memory/feedback_declarative_flows.md`
- `~/.claude/projects/-Users-luis-d-code-github/memory/feedback_no_shell.md`

## What To Do Next

1. **Check WH limit sweep results** — if 12×50m matches 3×200m, CPU limits are confirmed as sole mechanism
2. **Run GW CPU test** — `burst-forge flow investigate-gw-cpu`
3. **Fix max_nodes=10 in investigation configs** — change to 20 for full 1000-pod scenarios
4. **Pangea state alignment** — import ad-hoc resources into Terraform state (deferred)
5. **Node isolation test (Test 3)** — needs new worker node group in Pangea + k8s
6. **Update customer guidance** with webhook investigation conclusions
7. **Move private configs** (with Confluence keys, access IDs) to pleme-io/k8s repo
