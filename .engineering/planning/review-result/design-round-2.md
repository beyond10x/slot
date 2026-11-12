---
format: aep.planning-md/3
id: review-result:design-round-2
kind: review-result
status: active
title: Design critic, round 2
relations:
- reviews: epic:adoption-measured
- reviews: epic:desktop-headroom
- reviews: epic:publish-first-release
- reviews: story:admission-waits-for-memory
- reviews: story:agentplugins-slot-plugin-released
- reviews: story:all-sessions-on-slot
- reviews: story:atlas-catalog-adoption
- reviews: story:builds-run-at-low-cpu-weight
- reviews: story:disk-guard
- reviews: story:docs-site-live
- reviews: story:first-release-0-1-0
- reviews: story:gates-enrolment
- reviews: story:memory-max-is-tested
- reviews: story:memory-max-scope-carries-cpu-weight
- reviews: story:path-shim
- reviews: story:permanent-service-unit
- reviews: story:pilot-session-measured
- reviews: story:prometheus-metrics
- reviews: story:public-repository
- reviews: story:remote-mode-on-a-real-server
- reviews: story:test-execution-bounded-by-pool
revision: 1
---
needs-revision
story:path-shim — its `depends_on story:docs-site-live` records no shared file (docs-site-live's scope is `docusaurus.config.ts`, `product.json` and the workflows, and its acceptance holds with or without an `agent-integration.md` edit). The edge still puts the last workstation-adoption story, and so `epic:adoption-measured`'s close, behind `story:public-repository` (external blocker) and `story:gates-enrolment`. Drop that edge so the page edit lands before or after the deploy, or, if kept, name in `epic:adoption-measured` that the epic's order has these cross-epic edges — .engineering/planning/story/path-shim.md:12, .engineering/planning/story/path-shim.md:44, .engineering/planning/epic/adoption-measured.md:37
epic:publish-first-release — story 5 is described as the site "from the tagged source", but `story:docs-site-live` has no edge to `story:first-release-0-1-0` and deploys "of the deployed commit" on push to `main`, so the epic and story disagree about what the site is built from. Change the epic line to match the story or add the edge — .engineering/planning/epic/publish-first-release.md:36, .engineering/planning/story/docs-site-live.md:25

**What you read:** 30 artifacts (3 epics, 17 stories, the specification, 2 blockers, 4 round-1 review-results, 2 visions). I read every non-review body in full. Commands: `aep plan artifact list`, `graph`, `relations`, `validate` (reports valid), and `show` on each id. I walked all declared edges, about 140 including `reviews`, 17 of them `depends_on`. All stay inside the store, so there is no cycle. I traced about 20 acceptance reads to a producer in the set, and an edge records every one, directly or through a chain. The four round-1 design findings are fixed: gates-enrolment no longer reads the tag push, the bot-push wording is settled, the epic order line matches the edges, and CPU weight is split cleanly between the two stories.

**What you could not establish:**
- Unease, not a finding: the `depends_on` chain `test-execution-bounded-by-pool` → `disk-guard` → `memory-max-is-tested` → `memory-max-scope-carries-cpu-weight` → `admission-waits-for-memory` → `prometheus-metrics` → `path-shim` is serial throughout, and each edge states its reason (the shared `server/mod.rs` and `server/tests.rs`). The alternative is splitting those files, which is the drafter's trade-off.
- Out of my lane (scope): `story:prometheus-metrics` has no `decomposes` edge to any epic, so no epic covers it.
- Out of my lane (parallel safety): `memory-max-is-tested` depends on `disk-guard` only for `tests.rs` ordering, which is recorded.

```findings
- file: .engineering/planning/story/path-shim.md
  line: 12
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "its depends_on story:docs-site-live records no shared file (docs-site-live's scope excludes agent-integration.md and its acceptance holds regardless), yet it puts the last workstation-adoption story behind public-repository (external blocker) and gates-enrolment; drop the edge, or if kept, state the cross-epic edges in epic:adoption-measured's order line"
- file: .engineering/planning/epic/publish-first-release.md
  line: 36
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "story 5 is described as the site from the tagged source, but story:docs-site-live has no edge to story:first-release-0-1-0 and deploys the deployed commit on push to main; change the epic line to match the story or add the edge"
```
