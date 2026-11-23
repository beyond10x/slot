---
format: aep.planning-md/3
id: review-result:compute-cluster-scope-round-1
kind: review-result
status: active
title: Scope critic, compute cluster, round 1
relations:
- reviews: epic:compute-cluster
- reviews: story:cluster-design-and-spec-in-the-gate
- reviews: story:one-hand-started-node-measured
- reviews: story:host-config-file
- reviews: story:sync-holds-back-secrets
- reviews: story:daemon-knows-nodes
- reviews: story:node-health-poll
- reviews: story:cluster-view-in-web-ui
- reviews: story:placement-sticks-to-a-node
- reviews: story:node-leaves-without-stranding-jobs
- reviews: story:daemon-conforms-to-cluster-spec
- reviews: story:provider-contract-and-node-up-down
- reviews: story:node-boot-script
- reviews: story:spot-nodes-and-reclaim
- reviews: story:results-come-back-from-a-node
- reviews: story:sync-sends-the-git-file-list
- reviews: story:nodes-start-on-queue-pressure
- reviews: story:warm-caches-across-node-lives
- reviews: story:new-worktree-seeded-from-a-sibling
revision: 1
---
needs-revision
epic:compute-cluster — the Acceptance's night with two remote nodes (local queue wait p90 under 5 minutes, median overhead under 2 s, at least half of rust CPU seconds remote, under 1 % fallbacks/reruns/lost nodes, no contradicted test result, cost within the cap) is promised and no drafted item claims measuring it; the nearest candidate is a new measured story like `story:pilot-session-measured`, depending on `story:nodes-start-on-queue-pressure` and `story:warm-caches-across-node-lives` — .engineering/planning/epic/compute-cluster.md:111
story:placement-sticks-to-a-node — acceptance 7 lists `node`, `sync_ms`, `remote_id` but drops `fetch_ms`, which design § 3.7 names beside `sync_ms` and § 9.5 uses for the 2 s overhead target; no story claims `fetch_ms`, so the epic's overhead measure has no source field — .engineering/planning/story/placement-sticks-to-a-node.md:70

**What you read.** 19 artifacts: the epic and the 18 drafted stories, whole bodies, with `aep plan artifact show` on each. I also ran `aep plan artifact graph` and `list`, and read design `docs/design/remote-pool.md` § 3.4, § 3.5, § 3.7, § 4.7, § 5, § 7.5 and § 9, and grepped `spec/domains/*.yaml` for `fetch_ms`, the cost cap, `[auto]`, and the `return`/`fetch`/`include` shapes.
- Promises extracted from the epic: 23. Outcome 5, specification and topology 4, operator decisions 1 to 7 (7), exclusions respected 1, acceptance 6.
- Traced to an item: 21. The two exceptions are the findings above.

**Duplicate claims and reach, checked and not counted**
- Every story is named in the epic's own list. Reach beyond the parent could only show in an acceptance, and I found none that lands in an exclusion of § "Not in this epic".
- `story:sync-holds-back-secrets` uses § 4.4 only for nested `.git` and path values, not the excluded out-of-tree path dependencies and private git databases.
- `story:results-come-back-from-a-node` stays on `Cargo.lock`, the `return` patterns and the opt-in's `fetch`, so it does not build `--fetch auto` or the `cargo run` split.
- `story:spot-nodes-and-reclaim` and `story:node-leaves-without-stranding-jobs` both rerun a lost node's jobs locally. The spot story only adds the Reclaimed reason and the notice, and depends on the other, so I did not count one outcome as claimed twice.
- `story:daemon-knows-nodes` (acceptance 4) and `story:placement-sticks-to-a-node` (acceptance 7) both put `node` in the job record. One covers `local`, the other the remote value, so this is incremental, not a duplicate.

**What you could not establish**
- No drafter's report was given to me, so I could not tell whether the night measurement was deliberately left to the operator at epic close. The epic does not say so, so I counted it as a gap.
- Out of my lane, so they set no verdict:
  - `story:cluster-design-and-spec-in-the-gate` acceptance 5 changes the CI path list of `AGENTS.md` (n), which the epic does not ask for (parallel-safety or design).
  - `story:daemon-conforms-to-cluster-spec` names `cluster.yaml` and `hostconfig.yaml` only, so `provisioning.yaml` has no conformance claim. This is a design or acceptance call.
  - The provider daily cost cap and the `[auto]` daily budget in `story:nodes-start-on-queue-pressure` are two separate figures (design).
  - `story:cluster-design-and-spec-in-the-gate`, `story:one-hand-started-node-measured` and `story:warm-caches-across-node-lives` all edit `docs/design/remote-pool.md` (parallel-safety).

```findings
- file: .engineering/planning/epic/compute-cluster.md
  line: 111
  category: scope
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "epic:compute-cluster — the Acceptance's night with two remote nodes (local queue wait p90 under 5 minutes, median overhead under 2 s, at least half of rust CPU seconds remote, under 1 % fallbacks/reruns/lost nodes, no contradicted test result, cost within the cap) is promised and no drafted item claims measuring it; the nearest candidate is a new measured story like story:pilot-session-measured, depending on story:nodes-start-on-queue-pressure and story:warm-caches-across-node-lives"
- file: .engineering/planning/story/placement-sticks-to-a-node.md
  line: 70
  category: scope
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "story:placement-sticks-to-a-node — acceptance 7 lists node, sync_ms, remote_id but drops fetch_ms, which design § 3.7 names beside sync_ms and § 9.5 uses for the 2 s overhead target; no story claims fetch_ms, so the epic's overhead measure has no source field"
```
