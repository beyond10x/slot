---
format: aep.planning-md/3
id: review-result:parallel-safety-round-1
kind: review-result
status: active
title: Parallel-Safety critic, round 1
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
story:gates-enrolment — acceptance 3 reads a tag push that story:first-release-0-1-0 makes, and the only edge runs the other way (first-release depends_on gates-enrolment), so the line cannot be checked when this story lands. Drop the line or move it to first-release — .engineering/planning/story/gates-enrolment.md:35
story:disk-guard — it and story:admission-waits-for-memory both change admission in `crates/slot/src/server/mod.rs`, `ServeArgs` in `cli.rs`, `types.rs`, `server/tests.rs` and the regenerated `cli.md` and `ui/src/generated/types.ts` (cited, both Scope sections). Neither body mentions the other, and admission's only edge is to story:memory-max-is-tested. Remedies: an ordering edge recording the shared files, or splitting the surface — .engineering/planning/story/disk-guard.md:52, .engineering/planning/story/admission-waits-for-memory.md:55
story:memory-max-scope-carries-cpu-weight — it and story:memory-max-is-tested both change the `memory_max` wrapper branch of `crates/slot/src/server/mod.rs` and `server/tests.rs` (cited, both bodies). Its only edge is to story:builds-run-at-low-cpu-weight, so nothing orders it against memory-max-is-tested. Remedies: an ordering edge naming the shared file, or splitting the surface — .engineering/planning/story/memory-max-scope-carries-cpu-weight.md:28, .engineering/planning/story/memory-max-is-tested.md:27
story:disk-guard — it and story:memory-max-is-tested or story:memory-max-scope-carries-cpu-weight also share `crates/slot/src/server/mod.rs` and `server/tests.rs`, and the body admits no overlap (cited). The `waves` output puts disk-guard in wave 1 and memory-max-is-tested in wave 2 with no recorded reason, so a wave number is not a recorded dependency. Remedies: an ordering edge naming the file, or splitting the surface — .engineering/planning/story/disk-guard.md:52, .engineering/planning/story/memory-max-is-tested.md:41
story:prometheus-metrics — it has no relation to any headroom story, yet it edits `crates/slot/src/server/mod.rs` (counters) and `server/tests.rs` (cited) beside story:admission-waits-for-memory, story:disk-guard, story:memory-max-is-tested, story:memory-max-scope-carries-cpu-weight and story:path-shim. It also counts job states that admission (wait reason `memory`) and disk-guard (a refused state) change. Remedies: ordering edges that record the shared file, or splitting the surface — .engineering/planning/story/prometheus-metrics.md:56
story:prometheus-metrics — it adds a "metrics page" under `website/docs/`, and `website/sidebars.ts` lists every doc page by name, so a new page needs an edit to `sidebars.ts`. That file appears in no Scope section (inferred from `website/sidebars.ts` listing items explicitly). It collides with story:docs-site-live, whose scope is all of `website/` — .engineering/planning/story/prometheus-metrics.md:60
story:test-execution-bounded-by-pool — its Scope says `server/mod.rs` "(token accounting, if tokens are held for the test phase)", which is the area admission edits in story:admission-waits-for-memory and story:disk-guard. The body does not say whether it touches that file, so the collision depends on an undecided design choice (cited, conditional). It should state the decision or the ordering — .engineering/planning/story/test-execution-bounded-by-pool.md:52
story:path-shim — it changes `look_path` and shim exclusion in `crates/slot/src/server/mod.rs`, plus `server/tests.rs` by implication (cited). That shares a file with story:admission-waits-for-memory, story:disk-guard, story:memory-max-is-tested, story:memory-max-scope-carries-cpu-weight, story:prometheus-metrics and story:test-execution-bounded-by-pool, and its chain of edges runs only through the adoption epic. Remedies: an ordering edge to those stories, or splitting the surface — .engineering/planning/story/path-shim.md:54
story:docs-site-live — its Scope is `website/` as a whole, and `website/docs/agent-integration.md` is the page story:path-shim edits (cited, both bodies). Neither body mentions the other. The body also says that page "exists at adoption" is false, because the file is in the tree now (`website/docs/agent-integration.md`, 61 lines). `website/` as a whole also collides with every story that writes under `website/docs/`. Narrow the scope to named files, and add an edge or split — .engineering/planning/story/docs-site-live.md:44, .engineering/planning/story/path-shim.md:55
story:public-repository — its Scope is "the first commit of this tree", the whole working tree, so it collides with every story that changes the tree. The body does not say which tree state the first commit freezes or whether the headroom stories land before or after it (cited). Name the freeze point or order it against them — .engineering/planning/story/public-repository.md:38

**What you read:** 24 artifacts (the 18 stories in full, the three epics, the specification, the two blockers and two visions by graph only) with `aep plan artifact waves --kind story`, `aep plan artifact graph` and `aep plan artifact show` on each story. I also read `website/sidebars.ts`, `website/docs/` and `crates/`, and ran `git status`.

**Surfaces (parts 3 and 4):** I established 14 stories' surfaces as cited, 0 as inferred (the sidebar finding is the one inferred claim), and 4 as no repository surface (agentplugins-slot-plugin-released, all-sessions-on-slot, atlas-catalog-adoption, pilot-session-measured, each declared "no change in this repository"). The CLI lists seven as unassessed, three of which I placed by their cited bodies (gates-enrolment, public-repository, remote-mode-on-a-real-server). I traced 13 acceptance reads to a producer in the set, and `depends_on` records 12 of them. The unrecorded one is the gates-enrolment tag push, listed above.

**What I could not establish:** none for my verdict. I did not judge two things that belong to other critics:
- Out of my lane (acceptance): story:admission-waits-for-memory says it depends on memory-max-is-tested only "for the shared test fixture". That is not a file collision, and it is for the design critic to judge.
- Out of my lane (scope or design): story:builds-run-at-low-cpu-weight and story:permanent-service-unit share `contrib/slot.service` and `getting-started.md`. They are ordered by a `depends_on` edge, so I raise no finding.
- Out of my lane (design): the generated `cli.md` and `ui/src/generated/types.ts` collide whichever story regenerates them. Whether regeneration belongs in each story is a design question.

```findings
- file: .engineering/planning/story/gates-enrolment.md
  line: 35
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "acceptance 3 reads a tag push made in story:first-release-0-1-0, which depends_on this story, so the reader has no edge to its producer and the edge that exists runs the other way; drop the line or move it to first-release"
- file: .engineering/planning/story/disk-guard.md
  line: 52
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "cited: story:admission-waits-for-memory (admission-waits-for-memory.md:55) lands on the same admission code in server/mod.rs, cli.rs ServeArgs, types.rs, server/tests.rs and the generated cli.md and generated types.ts; neither body mentions the other; remedies are an ordering edge recording the shared files or splitting the surface"
- file: .engineering/planning/story/memory-max-scope-carries-cpu-weight.md
  line: 28
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "cited: story:memory-max-is-tested (memory-max-is-tested.md:27) and this story both change the memory_max wrapper branch in server/mod.rs and server/tests.rs; this story's only edge is to builds-run-at-low-cpu-weight; remedies are an ordering edge naming the shared file or splitting the surface"
- file: .engineering/planning/story/disk-guard.md
  line: 52
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "cited: story:memory-max-is-tested and story:memory-max-scope-carries-cpu-weight share server/mod.rs and server/tests.rs with disk-guard and no body admits it; the waves output (wave 1 against wave 2) records no reason for the order; remedies are an ordering edge naming the file or splitting the surface"
- file: .engineering/planning/story/prometheus-metrics.md
  line: 56
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "cited: the story has no relation to any headroom story yet edits server/mod.rs and server/tests.rs beside admission-waits-for-memory, disk-guard, memory-max-is-tested, memory-max-scope-carries-cpu-weight and path-shim, and counts job states that admission and disk-guard change; remedies are ordering edges recording the shared file or splitting the surface"
- file: .engineering/planning/story/prometheus-metrics.md
  line: 60
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "inferred: a new metrics page under website/docs needs an edit to website/sidebars.ts, which lists doc pages explicitly and is in no Scope section; it collides with docs-site-live, whose scope is all of website/"
- file: .engineering/planning/story/test-execution-bounded-by-pool.md
  line: 52
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "cited, conditional: Scope names server/mod.rs 'if tokens are held for the test phase', the token accounting area admission-waits-for-memory and disk-guard also edit; the body does not decide whether it touches the file, so the collision is undeclared"
- file: .engineering/planning/story/path-shim.md
  line: 54
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "cited: look_path and shim exclusion in server/mod.rs share a file with admission-waits-for-memory, disk-guard, memory-max-is-tested, memory-max-scope-carries-cpu-weight, prometheus-metrics and test-execution-bounded-by-pool, and path-shim's edges run only through the adoption epic; remedies are an ordering edge or splitting the surface"
- file: .engineering/planning/story/docs-site-live.md
  line: 44
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "cited: Scope is all of website/ and the agent-integration page is also edited by story:path-shim (path-shim.md:55), neither body mentions the other; the body's claim at line 30 that website/docs/agent-integration.md does not exist is false in the tree now; narrow the scope to named files and add an edge or split"
- file: .engineering/planning/story/public-repository.md
  line: 38
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "cited: Scope is the first commit of the whole tree, which collides with every story that changes the tree, and the body does not say which tree state it freezes or how it orders against the headroom stories"
```
