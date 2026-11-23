---
format: aep.planning-md/3
id: review-result:compute-cluster-design-round-1
kind: review-result
status: active
title: Design critic, compute cluster, round 1
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
story:cluster-view-in-web-ui — its acceptance 1 and 3 read placements and the PlacementsOnNode view that story:placement-sticks-to-a-node specifies and creates (placement.rs, types.rs), and no depends_on edge runs from it to that story; add depends_on story:placement-sticks-to-a-node (the cluster view then lands after placement, or the Placement types are moved into story:daemon-knows-nodes) — .engineering/planning/story/cluster-view-in-web-ui.md:34,45-46 and .engineering/planning/story/placement-sticks-to-a-node.md:48
story:new-worktree-seeded-from-a-sibling — its acceptance 3 reads "if phase 0 found the defect", the result of the seeding-across-commits check that story:one-hand-started-node-measured records in its verification-report, and no depends_on path from it reaches that story; add depends_on story:one-hand-started-node-measured — .engineering/planning/story/new-worktree-seeded-from-a-sibling.md:51 and .engineering/planning/story/one-hand-started-node-measured.md:48-49
story:provider-contract-and-node-up-down — its acceptance 8 reads the pattern list that story:cluster-design-and-spec-in-the-gate publishes in its pull request, and no depends_on path from it reaches that story; add depends_on story:cluster-design-and-spec-in-the-gate — .engineering/planning/story/provider-contract-and-node-up-down.md:75 and .engineering/planning/story/cluster-design-and-spec-in-the-gate.md:43-45
story:results-come-back-from-a-node — the opt-in's `return` and `fetch` patterns are read here but set by no story: story:placement-sticks-to-a-node owns `slot node allow <repository>`, which takes only the repository, this story's scope lists no cli.rs, and story:sync-sends-the-git-file-list adds `include` to the same empty `Repository` entity, so the opt-in is split over three stories with no owner of the command that sets its patterns; give this story (or placement) the allow-command option and one body the whole opt-in shape — .engineering/planning/story/results-come-back-from-a-node.md:34,47 and .engineering/planning/story/placement-sticks-to-a-node.md:60
story:daemon-conforms-to-cluster-spec — its acceptance 1 requires every synthesized scenario of all of cluster.yaml and hostconfig.yaml to pass, while story:results-come-back-from-a-node, story:sync-sends-the-git-file-list and story:nodes-start-on-queue-pressure each add fields or tables to those same spec files later and no edge orders them against this story; either record depends_on edges from this story to those three (conformance last) or name in the body that each of them keeps the suite green in its own change — .engineering/planning/story/daemon-conforms-to-cluster-spec.md:32,43 and .engineering/planning/story/results-come-back-from-a-node.md:34, .engineering/planning/story/sync-sends-the-git-file-list.md:31, .engineering/planning/story/nodes-start-on-queue-pressure.md:32

What I read: 18 stories + epic:compute-cluster + executable-system-specification:compute-cluster in full (`aep plan artifact show` each), `aep plan artifact relations`, `aep plan artifact graph` (whole store), `aep plan artifact validate` (valid, 97 artifacts), and spec/domains/{cluster,hostconfig,provisioning}.yaml by grep for the fields the acceptances read.

What I could not establish:
- Cycle: none. I walked 24 `depends_on` edges inside the set plus the 3 per-story decomposes/serves/informed_by edges, and the whole graph including the 4 blockers (decision-blocker, dependency-blocker) and the 40+ stories outside the set. No `depends_on` runs outside the set; the blockers only `block` placement-sticks-to-a-node, one-hand-started-node-measured and unrelated stories.
- Chain: the set is not a queue. The longest path is host-config, daemon-knows-nodes, node-health-poll, placement, node-leaves, provider, spot or warm.
- Acceptance reads traced to a producer in the set: about 22. 17 are covered by a direct or transitive depends_on edge; the 5 above are not, and they are the findings.
- No horizontal slicing. Spec and design are committed first as one story, the rest cut by behaviour. cluster-view-in-web-ui is a UI layer over daemon-knows-nodes, and its Not in this story line admits the placement dependency above.
- Out of my lane or unease, set no verdict:
  - story:node-boot-script depends on story:one-hand-started-node-measured, and so do spot and warm transitively. No acceptance of node-boot-script reads that story's report; the edge has no reason written beside it, and it makes the script wait on the operator's paid-machine decision-blocker:phase-0-node-launch.
  - story:sync-sends-the-git-file-list depends on story:results-come-back-from-a-node with no reason written. This is probably the shared client.rs and cluster.yaml, which is the parallel-safety critic's call.
  - GoneReason Drained and Deadline (spec cluster.yaml:67) have no owner. node-health-poll does Lost, spot does Reclaimed, and nothing moves a Draining node to Gone when it is idle or past its deadline. That is scope coverage.
  - story:warm-caches-across-node-lives names a cache location in spec/domains/hostconfig.yaml that the spec does not have, and its scope lists no spec file. That is acceptance or scope.
  - story:node-health-poll's Specification says "placements end" on MarkNodeGone before any Placement exists. story:node-leaves-without-stranding-jobs owns that effect. Minor wording.

```findings
- file: .engineering/planning/story/cluster-view-in-web-ui.md
  line: 45
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "its acceptance 1 and 3 read placements and the PlacementsOnNode view that story:placement-sticks-to-a-node specifies and creates, and no depends_on edge runs from it to that story; add depends_on story:placement-sticks-to-a-node, or move the Placement types into story:daemon-knows-nodes"
- file: .engineering/planning/story/new-worktree-seeded-from-a-sibling.md
  line: 51
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "its acceptance 3 reads 'if phase 0 found the defect', the seeding-across-commits result that story:one-hand-started-node-measured records, and no depends_on path from it reaches that story; add depends_on story:one-hand-started-node-measured"
- file: .engineering/planning/story/provider-contract-and-node-up-down.md
  line: 75
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "its acceptance 8 reads the pattern list that story:cluster-design-and-spec-in-the-gate publishes, and no depends_on path from it reaches that story; add depends_on story:cluster-design-and-spec-in-the-gate"
- file: .engineering/planning/story/results-come-back-from-a-node.md
  line: 34
  category: design
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the opt-in's return and fetch patterns are read here but set by no story: placement's `slot node allow <repository>` takes only the repository, this story's scope lists no cli.rs, and sync-sends-the-git-file-list adds include to the same empty Repository entity, so the opt-in is split over three stories with no owner of the command that sets its patterns; give one body the whole opt-in shape and the allow-command option"
- file: .engineering/planning/story/daemon-conforms-to-cluster-spec.md
  line: 43
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "its acceptance 1 requires every synthesized scenario of all of cluster.yaml and hostconfig.yaml to pass, while results-come-back-from-a-node, sync-sends-the-git-file-list and nodes-start-on-queue-pressure each add fields or tables to those spec files later with no edge ordering them against it; record depends_on edges to those three or state that each keeps the suite green in its own change"
```
