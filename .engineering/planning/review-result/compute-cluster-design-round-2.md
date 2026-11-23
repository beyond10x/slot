---
format: aep.planning-md/3
id: review-result:compute-cluster-design-round-2
kind: review-result
status: active
title: Design critic, compute cluster, round 2
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
needs-revision

story:host-config-file — its acceptance 3 enumerates "one fixture per refusal of `slot.hostconfig`", a list that lives only in `spec/domains/hostconfig.yaml`, which story:cluster-design-and-spec-in-the-gate is the story that commits (`git ls-files spec` prints nothing, the spec is untracked), and no depends_on path reaches that story from host-config-file or from any story downstream of it (daemon-knows-nodes, node-health-poll, cluster-view-in-web-ui, placement-sticks-to-a-node, node-leaves-without-stranding-jobs). `aep plan artifact waves` puts it beside the design story in W1. Add depends_on story:cluster-design-and-spec-in-the-gate to host-config-file; that one edge covers the whole chain below it — .engineering/planning/story/host-config-file.md:35-39,55-57 and .engineering/planning/story/cluster-design-and-spec-in-the-gate.md:34

story:daemon-knows-nodes — the spec's AddRemoteNode and Node already carry `machine` (and a `launched_as` reference to slot.provisioning.Machine), and story:daemon-conforms-to-cluster-spec requires every cluster.yaml scenario to pass, but this story's Node and `slot node add` lists omit `machine`, while story:provider-contract-and-node-up-down is the one that names `machine` in AddRemoteNode, and it runs after the conformance story. AddRemoteNode is half-built in each story. The body should say that Node and AddRemoteNode carry an optional `machine`, empty until the provider story, or the conformance story should name it as not covered — spec/domains/cluster.yaml:141-143,163-165,508-509, .engineering/planning/story/daemon-knows-nodes.md:47-48,66-67 and .engineering/planning/story/provider-contract-and-node-up-down.md:49-50

What I read: 20 artifacts in full (epic, 19 stories), round 1's design review-result, the spec's cluster.yaml and hostconfig.yaml by grep and range. Commands: `aep plan artifact show` on the review-result, `relations`, `graph` (whole store), `validate` (valid, 102 artifacts), plus `cat -n` on the story files for line citations.

Round 1 against this round:
- All five round-1 findings landed.
  - cluster-view: the Placement types and views now sit in daemon-knows-nodes and answer `[]` (daemon-knows-nodes.md:73), and the edge placement -> cluster-view is recorded.
  - new-worktree -> one-hand-started-node-measured and provider -> cluster-design are now edges.
  - results now owns the whole opt-in shape (results-come-back-from-a-node.md:32-34,52).
  - daemon-conforms is ordered ahead of results, provider and (through provider) nodes-start and sync (daemon-conforms.md:70-71).
- The new story:cluster-night-measured has edges to nodes-start, warm, spot and new-worktree. Every producer its acceptance reads (provider, placement, sync, results, node-leaves) is reached by a path, so it adds no defect.

What I walked:
- 35 `depends_on` edges inside the set, plus the whole-store graph (`depends_on` edges only run inside the set; the 2 blockers only `block`).
- No cycle.
- No full chain: the longest path is host-config, daemon-knows, health-poll, cluster-view, placement, node-leaves, conforms, provider, nodes-start, warm, night, with branches off it.
- About 36 acceptance reads traced to a producer in the set. 34 are recorded by a direct or transitive edge. The 2 not covered are the findings above.
- No horizontal slicing.

What I could not establish:
- Unease, not a finding: story:cluster-view-in-web-ui depends_on story:node-health-poll, but its revised body says every field exists from daemon-knows-nodes and later stories only fill values (cluster-view-in-web-ui.md:41-43), so nothing in its acceptance reads health-poll output. The edge puts cluster-view on the spine, so placement sits two waves behind health-poll. Placement keeps its own edge to health-poll, so dropping this one costs nothing.
- Out of my lane, set no verdict:
  - story:cluster-night-measured says "Blocked on" the operator's spending decision (line 56), but no `blocks` edge or decision-blocker records it (scope or parallel-safety).
  - Node-health-poll's "Lost" in the spec also covers three failed syncs (cluster.yaml:67-68), and node-leaves acceptance 1 does not cover it (scope or acceptance).
  - The warm-caches story names `image` as a field it adds to hostconfig.yaml, while the spec already has `image` in AwsSettings (hostconfig.yaml:67) (acceptance or scope).

```findings
- file: .engineering/planning/story/host-config-file.md
  line: 55
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "its acceptance 3 enumerates the refusals of slot.hostconfig, a list that lives in spec/domains/hostconfig.yaml, which story:cluster-design-and-spec-in-the-gate commits (the spec is untracked today), and no depends_on path from it or from any story below it reaches that story; add depends_on story:cluster-design-and-spec-in-the-gate"
- file: .engineering/planning/story/daemon-knows-nodes.md
  line: 47
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the spec's Node and AddRemoteNode already carry `machine`, and story:daemon-conforms-to-cluster-spec requires every cluster.yaml scenario to pass, but this body's Node and `slot node add` omit it while story:provider-contract-and-node-up-down names `machine` in AddRemoteNode and runs after the conformance story, so AddRemoteNode is half-built in each; say here that Node and AddRemoteNode carry an optional `machine`, empty until the provider story, or name it as not covered in the conformance story"
```
