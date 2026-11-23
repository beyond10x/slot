---
format: aep.planning-md/3
id: review-result:compute-cluster-scope-round-2
kind: review-result
status: active
title: Scope critic, compute cluster, round 2
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
- reviews: story:cluster-night-measured
revision: 1
---
approve

**What you read.** 20 artifacts: the epic first (revision 2, `epic:compute-cluster`, 111 lines, via `aep plan artifact show`, promises written down before any item), then all 19 stories whole, including the new `story:cluster-night-measured`. I also read round 1's `review-result:compute-cluster-scope-round-1`. I ran `aep plan artifact list` and `graph` for other claimants of the night measures and found none outside this set: `story:pilot-session-measured` and `epic:adoption-measured` are another epic's. I read `docs/design/remote-pool.md` § 9.5 and grepped it for `--fetch`, `fetch_ms` and `sync_ms`. I grepped `spec/domains/cluster.yaml` for the same fields.

**Promises and tracing.** I extracted 23 promises from the epic: the outcome (5), spec and topology (4), decisions 1 to 7 (7), the exclusions (1), and the acceptance measures (6). All 23 trace to an item.

**Round 1 findings**
- The blocker is fixed. `story:cluster-night-measured` (`.engineering/planning/story/cluster-night-measured.md`, acceptance 1 to 6, lines 36 to 41) claims each of the epic's six measures at `.engineering/planning/epic/compute-cluster.md:98-102`: queue wait p90 under 5 minutes, median overhead under 2 s, half of rust CPU seconds remote, under 1 % fallbacks, reruns and lost nodes, no contradicted result, and cost within the cap. The epic names the story at line 98 and in wave item 14 at line 80. Its dependencies are the four right stories, and nothing else claims the night.
- The warning is fixed. `story:placement-sticks-to-a-node` acceptance 7 now lists `fetch_ms` (`.engineering/planning/story/placement-sticks-to-a-node.md:60`). `--fetch` is an existing flag (design line 56 and `crates/slot/src/cli.rs`), so the source field is real. `story:daemon-knows-nodes` acceptance 4 gives it its default.

**Checked and not counted**
- Duplicate claims: `story:cluster-night-measured` and `story:one-hand-started-node-measured` both measure overhead. The first is the epic's night on the built cluster and the second is phase 0 on today's remote mode, so the outcomes differ.
- Reach: the night story's p90 overhead under 5 s comes from design § 9.5 (line 870). The epic's own wording is "as § 9.5 says", so it is traceable. No story lands in the epic's "Not in this epic" list.
- Silent narrowing: none found. The epic names the `cargo run` split, `--fetch auto` and the workstation-path mirrors as not built, and no story builds them.

**What you could not establish**
- I was given no drafter's report. I read the night story's reliance on the operator's spending decision as a block, not a narrowing.
- Out of my lane (no effect on the verdict):
  - Spec and design lane: `story:cluster-night-measured` (line 24) names `fetch_ms` as a field of the spec's Jobs view, and `story:daemon-knows-nodes` acceptance 4 gives the job record `fetch_ms` and `remote_id`. `spec/domains/cluster.yaml` (Jobs view, line 1199, and the Job entity, line 255) declares only `sync_ms`. The stories also say `node` where the spec says `node_id`.
  - Parallel-safety lane: three stories edit `docs/design/remote-pool.md`: the night story, `story:cluster-design-and-spec-in-the-gate` and `story:one-hand-started-node-measured`. `story:warm-caches-across-node-lives` also edits it.
  - Acceptance lane: `story:cluster-night-measured` is blocked on a spending decision, and its body points at `decision-blocker:phase-0-node-launch`, which is the phase-0 launch. No decision-blocker for the night's spend is named.

```findings
[]
```
