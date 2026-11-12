---
format: aep.planning-md/3
id: review-result:design-round-1
kind: review-result
status: active
title: Design critic, round 1
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
gates-enrolment — its acceptance line 3 reads a tag push that `story:first-release-0-1-0` performs, while that story has a `depends_on` edge to gates-enrolment, so gates-enrolment cannot be checked until its own dependent lands; move line 3 into first-release-0-1-0's acceptance, or have gates-enrolment make its own test tag push — .engineering/planning/story/gates-enrolment.md:35, .engineering/planning/story/first-release-0-1-0.md:22
public-repository — its outcome and acceptance 2 require the first commit to be pushed by `b10x-bot[bot]`, but gates-enrolment's outcome is what makes `b10x-gates bot --repository beyond10x/slot` able to push, and gates-enrolment `depends_on` public-repository; the body does not say how the bot can push before enrolment — .engineering/planning/story/public-repository.md:16, .engineering/planning/story/gates-enrolment.md:17
epic:publish-first-release — its body says "3 and 5 need 1 and 2 (each story names its `depends_on`)" but `story:atlas-catalog-adoption` declares `depends_on` only `story:public-repository`, and its own body gives only that reason; change the epic line to match, or add the edge to `story:gates-enrolment` — .engineering/planning/epic/publish-first-release.md:38, .engineering/planning/story/atlas-catalog-adoption.md:26
story:builds-run-at-low-cpu-weight — its outcome says "Every job the daemon starts runs in a systemd cgroup with `CPUWeight=20`", but its own context hands jobs wrapped by `--memory-max` to `story:memory-max-scope-carries-cpu-weight`, so half of one abstraction (CPU weight on jobs) sits in each item; narrow the outcome to the daemon's default cgroup, or merge the two — .engineering/planning/story/builds-run-at-low-cpu-weight.md:22, .engineering/planning/story/builds-run-at-low-cpu-weight.md:30

**What you read:** 26 artifacts, every body in full. Commands: `aep plan artifact list`, `relations`, `graph` and `validate` (reports valid). I walked all declared edges (about 60, 12 of them `depends_on`) and none leave the set. I found no `depends_on` cycle. The longest chains are public-repository → gates-enrolment → first-release-0-1-0 → agentplugins-slot-plugin-released, and permanent-service-unit → pilot-session-measured → all-sessions-on-slot → path-shim. Both are ordered on purpose, with the reason written in each epic. I traced 14 acceptance reads to a producer in the set. 12 are recorded by an edge. The other two are the first two findings above, one an inverse read and one a read of state a later item grants.

**What you could not establish:**
- Whether the bot can push to a new organization repository before enrolment (public-repository finding). That is external knowledge, and I only saw the tension in the bodies.
- Out of my lane (scope): the acceptance of `epic:adoption-measured` asks for a five-day report taken after the shim is installed. No story produces it, because path-shim has no measurement acceptance.
- Out of my lane (parallel safety): `story:admission-waits-for-memory` depends on `story:memory-max-is-tested` "only for the shared test fixture", but the producer names no fixture. The mod.rs and tests.rs edits in the desktop-headroom stories overlap and carry no ordering edges, except the two CPU-weight stories.
- Out of my lane (parallel safety): both `story:path-shim` and `story:docs-site-live` touch the agent-integration page under `website/docs/` with no edge between them.

```findings
- file: .engineering/planning/story/gates-enrolment.md
  line: 35
  category: design
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "its acceptance line 3 reads a tag push that story:first-release-0-1-0 performs, while first-release-0-1-0 depends_on gates-enrolment, so gates-enrolment cannot be checked until its own dependent lands; move line 3 into first-release-0-1-0's acceptance or have gates-enrolment make its own test tag push"
- file: .engineering/planning/story/public-repository.md
  line: 16
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "its outcome and acceptance 2 require the first commit pushed by b10x-bot[bot], but gates-enrolment's outcome is what makes b10x-gates bot able to push and gates-enrolment depends_on public-repository; the body does not say how the bot can push before enrolment"
- file: .engineering/planning/epic/publish-first-release.md
  line: 38
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the epic says 3 and 5 need 1 and 2 and that each story names its depends_on, but story:atlas-catalog-adoption declares depends_on only story:public-repository; change the epic line to match or add the edge to story:gates-enrolment"
- file: .engineering/planning/story/builds-run-at-low-cpu-weight.md
  line: 22
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "its outcome says every job the daemon starts runs with CPUWeight=20, but its own context hands --memory-max-wrapped jobs to story:memory-max-scope-carries-cpu-weight, so half of one abstraction sits in each item; narrow the outcome to the daemon's default cgroup or merge the two"
```
