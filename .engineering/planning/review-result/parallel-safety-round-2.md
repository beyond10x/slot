---
format: aep.planning-md/3
id: review-result:parallel-safety-round-2
kind: review-result
status: active
title: Parallel-Safety critic, round 2
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
story:prometheus-metrics — the body omits the existing `crates/slot/src/server/metrics.rs` (cited: 270 lines, `//! Prometheus text exposition for GET /metrics`, with `Histogram`, `record` and `render`) and lists only `http.rs`, `mod.rs` and `tests.rs`; its Context says the router has three routes, but `http.rs:27` already has `.route("/metrics", get(metrics))` and `mod.rs:582` and `mod.rs:695` already call `metrics.record` and `metrics_text`, so the body must state what is already in the working tree and name `metrics.rs` as the file it changes — .engineering/planning/story/prometheus-metrics.md:43, .engineering/planning/story/prometheus-metrics.md:66
story:disk-guard — its Scope adds a refused job state in `types.rs` "if a refused state is added" but omits `crates/slot/src/server/metrics.rs`, which has an exhaustive `match` on `JobState` (`metrics.rs:72-77`); a new variant breaks that file's build, so disk-guard has to edit it and the body does not say so, which leaves a shared file with story:prometheus-metrics unnamed. Add the file to its scope and name the shared file as the reason for the order, or move the `JobState` change to another item — .engineering/planning/story/disk-guard.md:63

**What I read:** 21 artifacts (18 stories and 3 epics in full), plus the five round-1 review-results through the graph and the round-1 parallel-safety record in full. Commands: `aep plan artifact waves --kind story`, `aep plan artifact graph`, `aep plan artifact show` on every story and epic, `aep plan artifact list`. I also read `Cargo.toml`, `Taskfile.yml`, `website/sidebars.ts`, `website/docs/`, `crates/slot/src/server/{http,metrics}.rs` and `crates/slot/src/types.rs`.

All ten round-1 findings are resolved:
- `server/mod.rs` and `server/tests.rs` are now one total `depends_on` chain: test-execution → disk-guard → memory-max-is-tested → memory-max-scope → admission → prometheus → path-shim.
- The only other shared files, `contrib/slot.service` and `getting-started.md`, are ordered by the edge from builds-run-at-low-cpu-weight to permanent-service-unit.
- The gates-enrolment tag-push acceptance is gone.
- The `website/` scope is narrowed to named files.
- `sidebars.ts` is named.
- The freeze point of the first commit is stated.

**Surfaces (3):** 13 stories are cited, 0 are inferred, and 0 are unplaced.
- The 13 are admission, builds-low-cpu, disk-guard, docs-site-live, first-release, gates-enrolment, memory-max-is-tested, memory-max-scope, path-shim, permanent-service-unit, prometheus, test-execution and remote-mode. Remote-mode names only `website/docs/` in its Scope, which matches `remote-builds.md` and no other story edits it.
- 5 declare no surface in this repository (agentplugins, all-sessions, atlas, pilot, public-repository). The `waves` output lists them as unassessed because they have no repository surface, not because the surface is unknown. Their other surfaces (the `.engineering/planning` reports, outside repositories, GitHub) collide with nothing in the set.

**Acceptance reads (4):** I traced 15 reads to a producer in the set. `depends_on` records all 15, 3 of them transitively (path-shim's read of the pilot's before-numbers goes through all-sessions-on-slot, and prometheus's reads of disk-guard's refused state and admission's waiting state go through the chain).

**What I could not establish:**
- None for the verdict.
- Out of my lane (acceptance): prometheus-metrics acceptance 1 expects `result="success"` and `result="failure"`, while the existing `metrics.rs:72-77` labels are `ok`, `failed` and `cancelled`.
- Out of my lane (design): test-execution-bounded-by-pool changes the documented `RUST_TEST_THREADS` behaviour (`website/docs/reference/environment.md:33`, `website/docs/toolchains.md:30`), and its Scope names only `docs/design.md`. No other story edits those pages, so nothing collides.
- Out of my lane (design): the headroom stories do not say whether they land before or after 0.1.0. The tag commit is whatever tree exists then.

```findings
- file: .engineering/planning/story/prometheus-metrics.md
  line: 43
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "cited: the body lists only http.rs, mod.rs and tests.rs and omits the existing crates/slot/src/server/metrics.rs (270 lines of exposition code), and says the router lacks /metrics although http.rs:27 already routes it and mod.rs:582 and mod.rs:695 already call the recorder; the body must state what is already in the tree and name metrics.rs as a surface it changes"
- file: .engineering/planning/story/disk-guard.md
  line: 63
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "cited: a refused JobState variant in types.rs breaks the exhaustive match on JobState in crates/slot/src/server/metrics.rs:72-77, so disk-guard must edit metrics.rs, a file it does not list and which story:prometheus-metrics also changes; add the file to the scope and name it as the shared surface behind the order, or split the JobState change out"
```
