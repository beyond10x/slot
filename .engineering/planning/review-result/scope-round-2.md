---
format: aep.planning-md/3
id: review-result:scope-round-2
kind: review-result
status: active
title: Scope critic, round 2
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
- reviews: specification:delivered-baseline
revision: 1
---
needs-revision
specification:delivered-baseline — the request says "what exists today should be recorded as delivered work", but the Delivered table has no row for `contrib/slot.service` and `task service`, which `story:permanent-service-unit` and `story:builds-run-at-low-cpu-weight` cite as existing — .engineering/planning/specification/delivered-baseline.md:30

**What you read.** 29 artifacts: 3 epics, 18 stories, the specification, 2 blockers, 2 visions and the round-1 scope review. I ran `aep plan artifact graph`, `kinds`, `relations` and `show` on every epic, story, blocker, vision and the specification. I also read `review-result:scope-round-1`, `Taskfile.yml` and `contrib/`, and grepped `crates/slot/src` for `memory_max` and `RUST_TEST_THREADS`.
- I extracted 22 promises from the request. That is the 8 numbered items split into sub-promises, the later test-bound story with its doc-tests note, the delivered record, and the "each artifact needs an acceptance" rule.
- I traced 21 to an item. The 22nd is the delivered-record gap above, which is partial.
- Epic promises: 6 of 6 in `epic:publish-first-release`, 4 of 4 in `epic:adoption-measured`, 6 of 6 in `epic:desktop-headroom`.
- Round 1's three scope findings no longer hold:
  - The test-execution story now says the old behaviour "is what is replaced".
  - The all-sessions story lists flock removal under "Not in this story".
  - The memory-max story "adds tests, not behaviour".
- Duplicate claims: none. The five-day 90 % measurement appears in `story:all-sessions-on-slot`, `story:path-shim` and the epic acceptance. The two stories cover different periods (after the instruction, after the shim), and the epic acceptance only rolls up the shim story's.
- Reach beyond the request: none that I would change. `story:admission-waits-for-memory` shows a "memory" wait reason in the UI, which supports the requested feature, so I did not count it as reach.

**What you could not establish.**
- Out of my lane, so they set no verdict:
  - The acceptance of `epic:publish-first-release` checks visibility, release assets, the site and the plugin manifest. It does not check gates enrolment or the Atlas catalog, though its Outcome promises both (acceptance).
  - `story:prometheus-metrics` and `story:remote-mode-on-a-real-server` have no `decomposes` parent epic. They are claimed once each, so I did not count it as a coverage defect (design).
  - `story:permanent-service-unit` accepts "running again after a reboot" while its context says a user unit starts at login unless lingering is enabled (acceptance).

```findings
- file: .engineering/planning/specification/delivered-baseline.md
  line: 30
  category: scope
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the request says \"what exists today should be recorded as delivered work\", but the Delivered table has no row for `contrib/slot.service` and `task service`, which story:permanent-service-unit cites as existing (permanent-service-unit.md:26)"
```
