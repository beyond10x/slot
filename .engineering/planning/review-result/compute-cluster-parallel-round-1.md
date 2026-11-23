---
format: aep.planning-md/3
id: review-result:compute-cluster-parallel-round-1
kind: review-result
status: active
title: Parallel-safety critic, compute cluster, round 1
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
story:daemon-conforms-to-cluster-spec — shares `Taskfile.yml` and `spec/ess-inputs.yaml` with story:cluster-design-and-spec-in-the-gate, which adds the `ess specify validate` step and pins the `ess` release there; no depends_on edge, and the design body only excludes conformance; cited; add an edge recording the shared files, or split the surface — .engineering/planning/story/daemon-conforms-to-cluster-spec.md:14, .engineering/planning/story/cluster-design-and-spec-in-the-gate.md:52
story:daemon-conforms-to-cluster-spec — acceptance 1 passes every synthesized scenario of all of `cluster.yaml` and `hostconfig.yaml`, state produced by story:provider-contract-and-node-up-down (AddRemoteNode machine/price), story:spot-nodes-and-reclaim (Reclaimed), story:results-come-back-from-a-node and story:sync-sends-the-git-file-list (Repository patterns, added to the spec there), story:nodes-start-on-queue-pressure (`[auto]`) and story:warm-caches-across-node-lives (cache location), and the only edge runs to story:node-leaves-without-stranding-jobs, so add depends_on edges to those six, or merge — .engineering/planning/story/daemon-conforms-to-cluster-spec.md:32, .engineering/planning/story/results-come-back-from-a-node.md:34
story:results-come-back-from-a-node — edits `spec/domains/cluster.yaml` (adds the opt-in's return/fetch patterns; grep finds none in the file today), which story:cluster-design-and-spec-in-the-gate puts on `main` (it is untracked in the tree now), and no depends_on edge runs to it; add the edge — .engineering/planning/story/results-come-back-from-a-node.md:34, .engineering/planning/story/cluster-design-and-spec-in-the-gate.md:31
story:nodes-start-on-queue-pressure — edits `spec/domains/hostconfig.yaml` (the `[auto]` table, added here) that story:cluster-design-and-spec-in-the-gate commits, and no edge runs to it; add the edge — .engineering/planning/story/nodes-start-on-queue-pressure.md:31, .engineering/planning/story/cluster-design-and-spec-in-the-gate.md:31
story:nodes-start-on-queue-pressure — its new `crates/slot/src/server/autoscale.rs` needs a `mod` line and a spawn in `crates/slot/src/server/mod.rs`, which the scope omits; story:new-worktree-seeded-from-a-sibling and story:results-come-back-from-a-node edit that file and neither is ordered with it (inferred: the wiring is my reading of a new module); name the file or the hook — .engineering/planning/story/nodes-start-on-queue-pressure.md:49
story:warm-caches-across-node-lives — its Specification extends `spec/domains/hostconfig.yaml` (a cache location, absent from the file today) and its scope edits `crates/slot/src/hostconfig.rs`; story:nodes-start-on-queue-pressure edits both, both depend on story:provider-contract-and-node-up-down and nothing orders the two (cited); add an edge recording the shared files, or split them, and list the spec file in the scope — .engineering/planning/story/warm-caches-across-node-lives.md:34, .engineering/planning/story/nodes-start-on-queue-pressure.md:49
story:spot-nodes-and-reclaim — both this and story:warm-caches-across-node-lives edit `contrib/node-boot.sh` (cited, both scopes), both depend on story:node-boot-script, and neither says so nor is ordered with the other; add an ordering edge naming the file, or split the surface — .engineering/planning/story/spot-nodes-and-reclaim.md:52, .engineering/planning/story/warm-caches-across-node-lives.md:54
story:node-leaves-without-stranding-jobs — shares `crates/slot/src/client.rs` with story:results-come-back-from-a-node, story:sync-sends-the-git-file-list and story:new-worktree-seeded-from-a-sibling, and `crates/slot/src/server/mod.rs` with story:results-come-back-from-a-node and story:new-worktree-seeded-from-a-sibling (cited, scopes); all hang off story:placement-sticks-to-a-node as separate branches and none says so; add ordering edges naming the files, or split the surface — .engineering/planning/story/node-leaves-without-stranding-jobs.md:60, .engineering/planning/story/results-come-back-from-a-node.md:54
story:new-worktree-seeded-from-a-sibling — acceptance 3 reads whether "phase 0 found the defect", which is the seeding-across-commits result story:one-hand-started-node-measured records in its verification-report, and no depends_on edge runs to it (its ancestors are placement and the git file list only); add the edge — .engineering/planning/story/new-worktree-seeded-from-a-sibling.md:51, .engineering/planning/story/one-hand-started-node-measured.md:44
story:cluster-view-in-web-ui — its view lists placements (story:placement-sticks-to-a-node), `gone_reason` (story:node-health-poll) and a cloud node's location and machine type (story:provider-contract-and-node-up-down), and depends only on story:daemon-knows-nodes; the fixture in acceptance 1 stands in for all three; add the edges, or merge — .engineering/planning/story/cluster-view-in-web-ui.md:45, .engineering/planning/story/cluster-view-in-web-ui.md:65
story:cluster-view-in-web-ui — `ui/src/generated/types.ts` is regenerated whenever `crates/slot/src/types.rs` changes, which story:node-health-poll and story:placement-sticks-to-a-node also change, and neither lists it nor is ordered with this one (inferred from the `task generate` drift check); name the file in those scopes, or order them — .engineering/planning/story/cluster-view-in-web-ui.md:60
story:node-boot-script — creates `crates/slot/tests/node_boot.rs` inside `crates/slot/tests/`, the directory story:daemon-conforms-to-cluster-spec claims whole, and no edge orders them (inferred: neither names a shared `tests/main.rs`); name the test entry point each uses, or order them — .engineering/planning/story/node-boot-script.md:52, .engineering/planning/story/daemon-conforms-to-cluster-spec.md:56
story:cluster-design-and-spec-in-the-gate — edits `.github/workflows/gate.yml`, which story:gates-enrolment (other epic, scope `.github`, inferred) also owns, and neither names the other; order them or split the file — .engineering/planning/story/cluster-design-and-spec-in-the-gate.md:59, aep plan artifact waves --kind story --status draft
story:one-hand-started-node-measured — edits `website/sidebars.ts`, which story:prometheus-metrics (other epic) also edits, and neither names the other; order them — .engineering/planning/story/one-hand-started-node-measured.md:57, aep plan artifact waves --kind story --status draft
story:daemon-knows-nodes — shares `crates/slot/src/server/mod.rs`, `types.rs`, `cli.rs` and generated `cli.md` with story:disk-guard and story:admission-waits-for-memory, `server/mod.rs` with story:memory-max-scope-carries-cpu-weight, story:test-execution-bounded-by-pool and story:path-shim, and `server/http.rs` with story:prometheus-metrics (all other epics; the cluster side cited, the other side from their store scopes, inferred); no body names any of them; the same `server/mod.rs` is edited by five later cluster stories; order each pair, or split the surface — .engineering/planning/story/daemon-knows-nodes.md:73, aep plan artifact waves --kind story --status draft
story:placement-sticks-to-a-node — shares `website/docs/remote-builds.md` with story:remote-mode-on-a-real-server and `crates/slot/src/client.rs` with story:path-shim (other epics), and neither names it; order each pair, or split the surface — .engineering/planning/story/placement-sticks-to-a-node.md:75, aep plan artifact waves --kind story --status draft
story:sync-holds-back-secrets — shares `crates/slot/src/toolchain.rs` with story:test-execution-bounded-by-pool and `crates/slot/src/client.rs` with story:path-shim (other epics), and neither names it; order each pair, or split the surface — .engineering/planning/story/sync-holds-back-secrets.md:51, aep plan artifact waves --kind story --status draft

What I read: 18 drafted stories plus 9 colliding stories of other epics via `aep plan artifact show`, `aep plan artifact waves --kind story --status draft`, `aep plan artifact relations`; plus `ls`, `git ls-files spec`, `grep` and `sed` on the tree and `website/sidebars.ts`. Surfaces: 18 cited (every body has a Scope section naming paths; the store tags them all "inferred"), 0 inferred, 0 unplaced; 3 findings rest on inferred wiring or generated files and say so. Acceptance reads traced to a producer in the set: 33, of which 20 are recorded by a `depends_on` edge and 13 are not (view 3, conforms 7 including the design story, seeded 1, results 1, queue-pressure 1).

What I could not establish:
- Out of my lane (design critic): story:one-hand-started-node-measured and story:remote-mode-on-a-real-server both measure remote mode on a real machine, including warm-call overhead against G4, so they may both be marked done for one outcome.
- Out of my lane: story:one-hand-started-node-measured runs today's sync, which has no deny list, and has no edge to story:sync-holds-back-secrets.
- Out of my lane (acceptance or design critic): story:node-boot-script's `crates/slot/tests/node_boot.rs` is a separate test binary against `AGENTS.md` "one integration-test binary per crate".
- Out of my lane (scope critic): story:placement-sticks-to-a-node and story:node-leaves-without-stranding-jobs are "Blocked on" `UNMAPPED:` decisions.
- Generated files (`website/docs/reference/cli.md`, `ui/src/generated/types.ts`) are changed by every story that edits `cli.rs` or `types.rs`, but only story:daemon-knows-nodes and story:cluster-view-in-web-ui list them. I did not file this per story: the chain of edges orders most of them, and the unordered case is the view finding above.
- The other-epic collisions come from the store's `inferred` scopes; I read those bodies only as far as their Outcome and Context.

```findings
- file: .engineering/planning/story/daemon-conforms-to-cluster-spec.md
  line: 14
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "shares Taskfile.yml and spec/ess-inputs.yaml with story:cluster-design-and-spec-in-the-gate (cited), no depends_on edge and the design body only excludes conformance; add an ordering edge recording the shared files, or split the surface"
- file: .engineering/planning/story/daemon-conforms-to-cluster-spec.md
  line: 43
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "acceptance 1 reads the whole of cluster.yaml and hostconfig.yaml, state produced by provider-contract-and-node-up-down, spot-nodes-and-reclaim, results-come-back-from-a-node, sync-sends-the-git-file-list, nodes-start-on-queue-pressure and warm-caches-across-node-lives, and the only edge runs to node-leaves-without-stranding-jobs; add depends_on edges to those six or merge"
- file: .engineering/planning/story/results-come-back-from-a-node.md
  line: 34
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "edits spec/domains/cluster.yaml, which story:cluster-design-and-spec-in-the-gate puts on main (untracked in the tree now), with no depends_on edge to it; add the edge"
- file: .engineering/planning/story/nodes-start-on-queue-pressure.md
  line: 31
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "edits spec/domains/hostconfig.yaml that story:cluster-design-and-spec-in-the-gate commits, with no depends_on edge to it; add the edge"
- file: .engineering/planning/story/nodes-start-on-queue-pressure.md
  line: 49
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the new server/autoscale.rs needs a mod line and a spawn in crates/slot/src/server/mod.rs, which the scope omits, and results-come-back-from-a-node and new-worktree-seeded-from-a-sibling edit that file unordered with it; inferred from the new module, not cited"
- file: .engineering/planning/story/warm-caches-across-node-lives.md
  line: 34
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "extends spec/domains/hostconfig.yaml (cache location, absent today) without listing it in the scope, and edits crates/slot/src/hostconfig.rs, both also edited by nodes-start-on-queue-pressure, with no edge between them; add an ordering edge naming the files, or split them"
- file: .engineering/planning/story/spot-nodes-and-reclaim.md
  line: 52
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "both this and warm-caches-across-node-lives edit contrib/node-boot.sh (cited, both scopes), both depend on node-boot-script, and nothing orders the two; add an ordering edge naming the file, or split the surface"
- file: .engineering/planning/story/node-leaves-without-stranding-jobs.md
  line: 60
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "shares client.rs with results-come-back-from-a-node, sync-sends-the-git-file-list and new-worktree-seeded-from-a-sibling, and server/mod.rs with results-come-back-from-a-node and new-worktree-seeded-from-a-sibling (cited, scopes); they are separate branches off placement-sticks-to-a-node and none says so; add ordering edges naming the files, or split the surface"
- file: .engineering/planning/story/new-worktree-seeded-from-a-sibling.md
  line: 51
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "acceptance 3 reads whether phase 0 found the defect, the seeding-across-commits result one-hand-started-node-measured records, and no depends_on edge runs to it; add the edge"
- file: .engineering/planning/story/cluster-view-in-web-ui.md
  line: 45
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "lists placements, gone_reason and a cloud node's location and machine type, produced by placement-sticks-to-a-node, node-health-poll and provider-contract-and-node-up-down, while depending only on daemon-knows-nodes, with a fixture standing in for them; add the edges or merge"
- file: .engineering/planning/story/cluster-view-in-web-ui.md
  line: 60
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "ui/src/generated/types.ts is regenerated whenever types.rs changes, which node-health-poll and placement-sticks-to-a-node also change, and neither lists it nor is ordered with this story; inferred from the task generate drift check, not cited"
- file: .engineering/planning/story/node-boot-script.md
  line: 52
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "creates crates/slot/tests/node_boot.rs inside the directory daemon-conforms-to-cluster-spec claims whole, with no edge between them; inferred (no shared tests/main.rs is named), so name each story's test entry point or order them"
- file: .engineering/planning/story/cluster-design-and-spec-in-the-gate.md
  line: 59
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "edits .github/workflows/gate.yml, which story:gates-enrolment (other epic, scope .github, inferred) also owns, and neither names the other; order them or split the file"
- file: .engineering/planning/story/one-hand-started-node-measured.md
  line: 57
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "edits website/sidebars.ts, which story:prometheus-metrics (other epic) also edits, and neither names the other; order them"
- file: .engineering/planning/story/daemon-knows-nodes.md
  line: 73
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "shares server/mod.rs, types.rs, cli.rs and cli.md with disk-guard and admission-waits-for-memory, server/mod.rs with memory-max-scope-carries-cpu-weight, test-execution-bounded-by-pool and path-shim, and server/http.rs with prometheus-metrics (other epics; the other side inferred from store scopes), and names none of them; order each pair or split the surface"
- file: .engineering/planning/story/placement-sticks-to-a-node.md
  line: 75
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "shares website/docs/remote-builds.md with remote-mode-on-a-real-server and client.rs with path-shim (other epics; the other side inferred from store scopes), and names neither; order each pair or split the surface"
- file: .engineering/planning/story/sync-holds-back-secrets.md
  line: 51
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "shares toolchain.rs with test-execution-bounded-by-pool and client.rs with path-shim (other epics; the other side inferred from store scopes), and names neither; order each pair or split the surface"
```
