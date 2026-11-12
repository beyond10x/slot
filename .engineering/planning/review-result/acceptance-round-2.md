---
format: aep.planning-md/3
id: review-result:acceptance-round-2
kind: review-result
status: active
title: Acceptance critic, round 2
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
story:first-release-0-1-0 — acceptance 1 says the tag push "is the bot's" but cites `gh run list --json event,headBranch,conclusion`, whose fields (`gh run list --help`) include no actor, so the bot-push claim cannot be checked as written — .engineering/planning/story/first-release-0-1-0.md:35
story:path-shim — the outcome names `go list` and rust-analyzer's invocations as pass-through, but acceptance 2 checks only `cargo metadata` and `go env GOPATH`, so a shim that routes `go list` or rust-analyzer's calls through slot still passes — .engineering/planning/story/path-shim.md:52
story:agentplugins-slot-plugin-released — the outcome says the plugin gives the instruction to run builds as `slot <command…>` with `--local` as the escape, but neither acceptance item checks that content, so a plugin that only installs the CLI passes — .engineering/planning/story/agentplugins-slot-plugin-released.md:30
story:atlas-catalog-adoption — the outcome lists components, relations and the `ROADMAP.md` objectives (O6, O2), but acceptance 1 and 2 check only the repository subject, the release unit and the README row, and "the catalog's own check" names no command — .engineering/planning/story/atlas-catalog-adoption.md:30
story:docs-site-live — the outcome says the CLI reference served is the generated `cli.md` of the deployed commit, but acceptance 1 to 3 only check HTTP 200 and a green run, so a stale reference page passes — .engineering/planning/story/docs-site-live.md:24
story:admission-waits-for-memory — the outcome says a held job shows `memory` "in the UI and the job log", but acceptance 1 checks only the waiting state and reason in a test and nothing checks the UI or a job-log line — .engineering/planning/story/admission-waits-for-memory.md:35

What I read: all 27 artifacts (3 epics, 17 stories, 2 blockers, the specification and the 4 round-1 reviews). I used `aep plan artifact list` and `aep plan artifact show` on each, and read `review-result:acceptance-round-1`. I did not read `vision:O2` or `vision:O6`, which are outside the set. All five round-1 acceptance findings are fixed in the drafts, so none is repeated. I checked the tree with `rg`: `--local` exists in `crates/slot/src/cli.rs`, and the `gh run list` JSON field list has no actor.

What I could not establish:
- Whether `docs/catalog.md` § Repositories in atlas and the atlas catalog check command exist as acceptance 1 and 2 of `story:atlas-catalog-adoption` name them. I did not open the atlas tree.
- I did not run `aep plan artifact lifecycle <kind>`, so I did not check status names.
- "Within one admission tick" in `story:admission-waits-for-memory` has no matching symbol. The daemon has a 1 s `tokio` interval at `crates/slot/src/server/mod.rs:629`. I treated the phrase as observable.
- Out of my lane (scope): the title of `story:public-repository` says "holding this tree", but its outcome excludes the push. `story:gates-enrolment` acceptance 1 checks the head of `main`, so it stops holding once a later commit by someone else lands. That is design or scope.

```findings
- file: .engineering/planning/story/first-release-0-1-0.md
  line: 35
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "acceptance 1 says the tag push is the bot's but cites gh run list --json event,headBranch,conclusion, whose fields include no actor, so the bot-push claim cannot be checked as written"
- file: .engineering/planning/story/path-shim.md
  line: 52
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the outcome names go list and rust-analyzer's invocations as pass-through, but acceptance 2 checks only cargo metadata and go env GOPATH, so a shim that routes go list or rust-analyzer's calls through slot still passes"
- file: .engineering/planning/story/agentplugins-slot-plugin-released.md
  line: 30
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the outcome says the plugin gives the instruction to run builds as slot <command…> with --local as the escape, but neither acceptance item checks that content, so a plugin that only installs the CLI passes"
- file: .engineering/planning/story/atlas-catalog-adoption.md
  line: 30
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the outcome lists components, relations and the ROADMAP.md objectives (O6, O2), but acceptance 1 and 2 check only the repository subject, the release unit and the README row, and the catalog's own check names no command"
- file: .engineering/planning/story/docs-site-live.md
  line: 24
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the outcome says the CLI reference served is the generated cli.md of the deployed commit, but acceptance 1 to 3 only check HTTP 200 and a green run, so a stale reference page passes"
- file: .engineering/planning/story/admission-waits-for-memory.md
  line: 35
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the outcome says a held job shows memory in the UI and the job log, but acceptance 1 checks only the waiting state and reason in a test and nothing checks the UI or a job-log line"
```
