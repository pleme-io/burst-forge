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
- WH: scale-dependent — WH=3 for ≤300 pods, WH=5 for ≥500 pods (contention cliff at WH=8)
- **ROOT CAUSE (Bottleneck #17): Proactive cache subprocess bug in GW.**
  `curl_proxy/proactive_cache.go:157` creates `ProxyCmd{}` with `cliDirect=false`
  (Go zero value), bypassing `useCLIDirectly`. Background cache refresh goroutines
  spawn `timeout --signal SIGKILL 30 akeyless <cmd>` subprocesses even when the
  default config is in-process direct mode. Under burst, these subprocesses exceed
  30s and get SIGKILL'd (207-459 kills per burst), causing SaaS connectivity cycling
  and injection stalls. Fix: set `cliDirect: useCLIDirectly` on ProxyCmd in
  proactive_cache.go. Mitigations: `PROACTIVE_CACHE_WORKERS=1` or disable proactive cache.
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

### WH Limit Sweep (COMPLETE — bottleneck #16: admission contention)
Re-ran with max_nodes=20. All 5 scenarios completed 1000/1000 with meaningful data:

| Scenario | WH | CPU Limit | Time (s) | pods/s | vs baseline |
|----------|-----|-----------|----------|--------|-------------|
| baseline-3wh-200m | 3 | 200m | 136.2 | 7.3 | — |
| 12wh-50m | 12 | 50m | 149.4 | 6.7 | -8% |
| 12wh-100m | 12 | 100m | 313.0 | 3.2 | -56% |
| 12wh-no-limit | 12 | none | 266.9 | 3.7 | -49% |
| 12wh-200m | 12 | 200m | 230.6 | 4.3 | -41% |

**Finding:** WH penalty is NOT CFS throttling. More CPU per WH makes it *worse*.
50m (most constrained) is fastest at WH=12. Root cause is API server mutating
admission serialization — 12 webhooks create admission queue contention. Injection
stalled at ~484 pods for 60s during the no-limit scenario, consistent with API
server admission saturation.

**Contention curve (500 pods, WH=3→12, default CPU):**
WH=3: 76.5s | WH=4: 70.0s | **WH=5: 62.6s** | WH=6: 63.1s | WH=8: 107.7s | WH=10: STUCK | WH=12: 170.0s

**WH=5 validation (WH=3 vs WH=5 at two scales):**
- 300 pods: WH=3 wins (48.9s vs 73.7s) — less overhead at low scale
- 1000 pods: WH=5 wins (109.4s vs 147.4s, **+26%**)

**Recommendation:** WH optimal is scale-dependent:
- ≤300 pods: WH=3 (Cerebras customer stays at 3)
- ≥500 pods: WH=5 (new recommendation for large bursts)
- Never exceed WH=6 on shared nodes

### Cerebras-Optimal Validation (all scales confirmed)
| Scale | GW | WH | Time (s) | 100% injected |
|-------|----|----|----------|---------------|
| 1000 | 15 | 3 | 146.7 | yes |
| 750 | 11 | 3 | 107.4 | yes |
| 500 | 8 | 3 | 86.9 | yes |
| 300 | 5 | 3 | 48.8 | yes |
| 150 | 3 | 2 | 36.6 | yes |
| 50 | 1 | 1 | 24.3 | yes |

Published: https://akeyless.atlassian.net/wiki/spaces/~7120203936f1d3939b4810895c20eb2bc58ae4/pages/3978952714

### GW CPU Test (COMPLETE — bottleneck #15 confirmed: QPS, not CPU)
`burst-forge flow investigate-gw-cpu` — all 3 scenarios succeeded after container
name fix (`akeyless-api-gateway` → `api-gateway`).

| Scenario | GW CPU Limit | Time (s) | Throughput (pods/s) |
|----------|-------------|----------|---------------------|
| gw-500m | 500m | 153.8 | 6.5 |
| gw-1000m | 1000m | 142.5 | 7.0 |
| gw-no-limit | none | 147.9 | 6.8 |

**Finding:** GW CPU has minimal effect. Doubling limit gained only 7.7%.
Removing limit entirely was slightly *slower* than 1000m. **QPS=5 is the
permanent throughput ceiling, not CPU.** The GW is network/API-bound.

Published: https://akeyless.atlassian.net/wiki/spaces/~7120203936f1d3939b4810895c20eb2bc58ae4/pages/3979575298

### burst-forge Code Improvements (this session)
5 commits of improvements:
1. **Critical bugs fixed:** teardown timeout returns Err (was silent Ok), rate
   calc dedup (4→2 shared functions), Job secret counting wired, HTTP 200-299
2. **Configurable container names:** init_container_name, workload_container_name,
   webhook_container_name, gateway_container_name, secret_path_prefix — all with
   backward-compatible defaults. JSON patches use serde_json::json!() now.
3. **JSON export:** `output_dir` config field writes results-{timestamp}.json
4. **Flux backoff:** 2x poll interval on kubectl errors
5. **14 unit tests:** rate functions, config defaults, all 21 YAML configs parse

### Pangea State Alignment (BLOCKED — gem dependency fix needed)

**Blocker:** The eks-scale-test workspace flake can't resolve `pangea-architectures`
gem because:
1. `eks_scale_test.rb` was missing `require 'pangea/architectures'` (FIXED)
2. Gemfile was missing transitive deps (pangea-splunk, etc.) (FIXED — bundix done)
3. **Root cause:** The workspace flake's `self` only captures `workspaces/eks-scale-test/`
   but `pangea-architectures` is a `path: '../..'` gem pointing to the repo root.
   The Nix build can't resolve this relative path outside the flake source tree.

**Fix needed:** Either restructure the flake to use the parent repo as source,
or publish pangea-architectures to a gem server so the workspace doesn't need
path dependencies.

**Once unblocked**, the 5 imports with now-known IDs:
```bash
tofu import aws_eks_node_group.scale-test-burst scale-test:scale-test-burst
tofu import aws_subnet.scale-test-pods-0 subnet-0a9f66f70b24b1fed
tofu import aws_subnet.scale-test-pods-1 subnet-076e7e717c6390e53
tofu import aws_route_table_association.scale-test-pods-0 rtbassoc-0656f91fa101949b0
tofu import aws_route_table_association.scale-test-pods-1 rtbassoc-0ca1da76fa50b7d95
```
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
