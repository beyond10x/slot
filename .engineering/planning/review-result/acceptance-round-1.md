---
format: aep.planning-md/3
id: review-result:acceptance-round-1
kind: review-result
status: active
title: Acceptance critic, round 1
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
story:all-sessions-on-slot — the acceptance lists what the verification-report records but sets no threshold or target state, so a report showing few sessions or many leftover `flock` files still satisfies it, though the outcome says every session is instructed and the `flock` files and `CARGO_BUILD_JOBS` settings are removed — .engineering/planning/story/all-sessions-on-slot.md:30
story:pilot-session-measured — the outcome sets a floor of at least one working day and at least 50 builds, but the acceptance only requires the report to record sample sizes, so a 3-build report passes — .engineering/planning/story/pilot-session-measured.md:35
story:memory-max-is-tested — the bound "a `peak_rss` at or below the limit plus one sampling interval's growth" has no number, so two reviewers can compute different thresholds from the same `jobs.jsonl` — .engineering/planning/story/memory-max-is-tested.md:39
story:first-release-0-1-0 — the outcome moves `CHANGELOG.md` § 0.1.0 from "unreleased" to a date, but no acceptance item checks it, and `CHANGELOG.md:3` reads "## 0.1.0 — unreleased" today — .engineering/planning/story/first-release-0-1-0.md:35
story:remote-mode-on-a-real-server — item 5 only requires the overhead to be "compared with G4", so any measured value passes, though the outcome states G4 as under 1 s at median — .engineering/planning/story/remote-mode-on-a-real-server.md:43

What I read: all 25 artifacts, with `aep plan artifact list` and `aep plan artifact show <id>` for each. I checked the tree with `git grep`-style greps: all 12 tests named in `specification:delivered-baseline` exist, `Job.remote` exists, the `README.md` install line exists, and `CHANGELOG.md` reads "unreleased". In atlas, the O2 and O6 `ROADMAP.md` rows match the `vision:O2` and `vision:O6` first paragraphs, and `grep -n slot atlas/README.md` finds nothing today, so that acceptance has a real before state.

What I could not establish:
- Whether an acceptance written as a roll-up of several commands is one statement. I treated the epics' conjunctive acceptances and the numbered story acceptances as acceptable and did not flag them.
- I did not run `aep plan artifact lifecycle <kind>`, so I did not check status names.
- Out of my lane: `decision-blocker:remote-build-host` clears on `ssh <it> slot --version`, but nothing in the plan installs `slot` on that machine, and `slot` 0.1.0 is not yet released. That is for the design or scope critic.

```findings
- file: .engineering/planning/story/all-sessions-on-slot.md
  line: 30
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the acceptance lists what the verification-report records but sets no threshold or target state, so a report showing few sessions or many leftover flock files still satisfies it, though the outcome says every session is instructed and the flock files and CARGO_BUILD_JOBS settings are removed"
- file: .engineering/planning/story/pilot-session-measured.md
  line: 35
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the outcome sets a floor of at least one working day and at least 50 builds, but the acceptance only requires the report to record sample sizes, so a 3-build report passes"
- file: .engineering/planning/story/memory-max-is-tested.md
  line: 39
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the bound 'a peak_rss at or below the limit plus one sampling interval's growth' has no number, so two reviewers can compute different thresholds from the same jobs.jsonl"
- file: .engineering/planning/story/first-release-0-1-0.md
  line: 35
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the outcome moves CHANGELOG.md § 0.1.0 from 'unreleased' to a date, but no acceptance item checks it, and CHANGELOG.md:3 reads '## 0.1.0 — unreleased' today"
- file: .engineering/planning/story/remote-mode-on-a-real-server.md
  line: 43
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "item 5 only requires the overhead to be 'compared with G4', so any measured value passes, though the outcome states G4 as under 1 s at median"
```
