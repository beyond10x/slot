---
format: aep.planning-md/3
id: review-result:compute-cluster-acceptance-round-1
kind: review-result
status: active
title: Acceptance critic, compute cluster, round 1
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
story:cluster-design-and-spec-in-the-gate — acceptance 1 runs `git grep -n -i -E '<placeholder list>'` and puts the real pattern list "in the pull request", so two people cannot run the same check and get the same answer, and `story:provider-contract-and-node-up-down` acceptance 8 cites that list as if it were in the store; name the patterns in the body or in a committed file the check reads — .engineering/planning/story/cluster-design-and-spec-in-the-gate.md:43
story:results-come-back-from-a-node — the outcome promises that "the files the operator's opt-in names for every successful `cargo build`" come back, but acceptance 1 to 4 test only Cargo.lock, `return` patterns, refused names and control sequences, and acceptance 5 only that the spec declares `fetch`, so the `fetch` behaviour can be missing and the story still closes; add a check that a successful `cargo build` brings back a file named by `fetch` — .engineering/planning/story/results-come-back-from-a-node.md:45
story:nodes-start-on-queue-pressure — the outcome says the daemon never passes "the most hours per node the configuration sets", but acceptance 1 to 3 check only the absent `[auto]`, the daily budget and idle takedown, so the hours-per-node bound has no check, and neither does the new `[auto]` table validating in the spec; add both — .engineering/planning/story/nodes-start-on-queue-pressure.md:42
story:node-boot-script — the outcome promises the machine stops "at its deadline and after 30 idle minutes", but acceptance 2 checks only that `shutdown` is called first, and acceptance 4 only that "the idle watchdog appear[s] in the recorded calls", so a timer set to any time and a watchdog at any threshold both pass; name that the recorded `shutdown` call carries the deadline given and that the watchdog uses 30 minutes — .engineering/planning/story/node-boot-script.md:43
story:new-worktree-seeded-from-a-sibling — acceptance 3 ends "if phase 0 found the defect, the seed drops the workspace's own fingerprints and the test shows it", so when phase 0 found nothing the clause is empty and the sentence reads the same before and after; state what is true in each case, or make the check unconditional — .engineering/planning/story/new-worktree-seeded-from-a-sibling.md:50
story:sync-sends-the-git-file-list — the Specification says the opt-in's `include` paths are "added to the spec here", but no acceptance says the spec declares `include` and validates (`story:results-come-back-from-a-node` acceptance 5 does that for its own spec change), so the spec edit can be skipped and the story still closes — .engineering/planning/story/sync-sends-the-git-file-list.md:38

What I read: 18 of 18 ids, with `aep plan artifact show` on each. I also ran `aep plan artifact kinds` and `aep plan artifact lifecycle story`, and checked `spec/ess-inputs.yaml`, `ess specify` and the design's fetch lines in the tree.

What I could not establish:
- `story:placement-sticks-to-a-node` and `story:node-leaves-without-stranding-jobs` carry `UNMAPPED:` blockers (repository key, rerun count); this is outside my lane, so it does not set my verdict.
- `story:cluster-design-and-spec-in-the-gate` acceptance 5 depends on a planning-only path list that exists only as open work (n); that is the parallel-safety and scope critics' lane.
- I could not check the stand-in ssh node or the placement-frame symbols, since they are new code.
- I did not flag the numbered multi-criterion acceptance lists as "more than one statement". Each item is separately observable, and flagging all 18 would be noise.

```findings
- file: .engineering/planning/story/cluster-design-and-spec-in-the-gate.md
  line: 43
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "acceptance 1 runs git grep over '<placeholder list>' and puts the real pattern list in the pull request, so the check is not reproducible from the store and story:provider-contract-and-node-up-down acceptance 8 cites a list that is not there"
- file: .engineering/planning/story/results-come-back-from-a-node.md
  line: 45
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the outcome promises files named by the opt-in's fetch patterns come back after every successful cargo build, but acceptance 1 to 5 check no behaviour of fetch, only that the spec declares it"
- file: .engineering/planning/story/nodes-start-on-queue-pressure.md
  line: 42
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the outcome says the daemon never passes the most hours per node the configuration sets, but no acceptance checks that bound or that the new [auto] table validates in the spec"
- file: .engineering/planning/story/node-boot-script.md
  line: 43
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "acceptance 2 checks only that shutdown is called first and acceptance 4 only that the idle watchdog appears in the recorded calls, so a timer at any time and a watchdog at any threshold pass; the deadline and the 30 idle minutes the outcome promises are not checked"
- file: .engineering/planning/story/new-worktree-seeded-from-a-sibling.md
  line: 50
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "acceptance 3 is conditional ('if phase 0 found the defect, the seed drops the workspace's own fingerprints'), so when phase 0 found nothing the clause is empty and reads the same before and after the work"
- file: .engineering/planning/story/sync-sends-the-git-file-list.md
  line: 38
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the Specification adds the opt-in's include paths to the spec in this story, but no acceptance says the spec declares include and validates, so the spec change can be skipped and the story still closes"
```
