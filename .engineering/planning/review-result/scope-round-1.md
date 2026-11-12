---
format: aep.planning-md/3
id: review-result:scope-round-1
kind: review-result
status: active
title: Scope critic, round 1
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
story:test-execution-bounded-by-pool — the request fixes the mechanism ("test binaries run with RUST_TEST_THREADS / NEXTEST_TEST_THREADS = free tokens at job start without holding those tokens"), but the story's scope allows holding tokens for the test phase and the body never says it departs from the stated mechanism — .engineering/planning/story/test-execution-bounded-by-pool.md:52
story:all-sessions-on-slot — the outcome also removes per-session `flock` slot files and hand-set `CARGO_BUILD_JOBS` from workspace scripts; the request says only "then the remaining sessions" and the epic's outcome does not mention it — .engineering/planning/story/all-sessions-on-slot.md:18
story:memory-max-is-tested — the outcome adds a new client stderr line ("the job hit `MemoryMax`") and an exit message for an OOM-killed scope; the request calls `--memory-max` untested, which asks for verification, not new behaviour — .engineering/planning/story/memory-max-is-tested.md:43

**What you read.** 27 artifacts: 3 epics, 18 stories, the specification, 2 blockers and 2 visions. Commands: `aep plan artifact graph`, `kinds`, `relations`, and `cat -n` of every file under `.engineering/planning`. I also read `AGENTS.md` and the heading list of `docs/design.md`.
- Promises I extracted from the request: 20. That is the 8 numbered items split into their sub-promises, plus the later test-bound story with its doc-tests note, plus the delivered record. I traced all 20 to an item.
- Epic promises: 6 of 6 in `epic:publish-first-release`, 4 of 4 in `epic:adoption-measured`, and 4 of 4 in `epic:desktop-headroom`.
- Item 4 of the epic list is "only partly traced": `story:test-execution-bounded-by-pool` claims its outcome but not the stated mechanism (first finding).
- Duplicate claims: none. Reach beyond the parent: the second and third findings.

**What you could not establish.**
- Out of my lane, so they set no verdict:
  - The epic `epic:publish-first-release` says stories 3 and 5 "need 1 and 2". `story:atlas-catalog-adoption` depends only on `story:public-repository`, not on `story:gates-enrolment` (design).
  - `story:builds-run-at-low-cpu-weight` in `epic:desktop-headroom` depends on `story:permanent-service-unit`, which belongs to `epic:adoption-measured` (design and parallel-safety).
  - The thresholds (90 %, 75 %, 50 builds) are the drafter's, not the request's (acceptance).
- The "Prometheus metrics" histograms carry a `toolchain` label that the request did not state. I treated it as a design choice, not reach.

```findings
- file: .engineering/planning/story/test-execution-bounded-by-pool.md
  line: 52
  category: scope
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the request fixes the mechanism (\"test binaries run with RUST_TEST_THREADS / NEXTEST_TEST_THREADS = free tokens at job start without holding those tokens\"), but the story's scope allows holding tokens for the test phase and the body never says it departs from the stated mechanism"
- file: .engineering/planning/story/all-sessions-on-slot.md
  line: 18
  category: scope
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the outcome also removes per-session flock slot files and hand-set CARGO_BUILD_JOBS from workspace scripts; the request says only \"then the remaining sessions\" and the epic's outcome does not mention it"
- file: .engineering/planning/story/memory-max-is-tested.md
  line: 43
  category: scope
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the outcome adds a new client stderr line and an exit message for an OOM-killed scope; the request calls --memory-max untested, which asks for verification, not new behaviour"
```
