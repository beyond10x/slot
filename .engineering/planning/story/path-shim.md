---
format: aep.planning-md/3
id: story:path-shim
kind: story
status: draft
title: cargo and go on PATH route through slot without catching metadata calls
relations:
- decomposes: epic:adoption-measured
- serves: vision:O6
- depends_on: story:all-sessions-on-slot
- depends_on: story:prometheus-metrics
scope:
- confidence: inferred
  path: contrib/shim
- confidence: inferred
  path: crates/slot/src/client.rs
- confidence: inferred
  path: crates/slot/src/server/mod.rs
- confidence: inferred
  path: crates/slot/src/server/tests.rs
- confidence: inferred
  path: website/docs/agent-integration.md
revision: 7
---
# Story: path-shim

## Outcome

A shim directory placed first on `PATH` provides `cargo` and `go` that run `slot cargo …` /
`slot go …`, so a session builds through the pool whether or not it follows the instruction. Calls
that must stay local and instant — `cargo metadata`, rust-analyzer's invocations, `go env`,
`go list` — pass straight to the real tool. The daemon never resolves the shim when it starts the
real command, so there is no recursion.

## Context

- `docs/design.md` § 4 rejected a shim for phase 1 because a silent shim also catches
  rust-analyzer and tools that run cargo for metadata; this story has to answer that, not ignore it.
- Local jobs get the client's full environment (`docs/design.md` § 10), including a `PATH` with
  the shim first; the daemon resolves the program with `look_path` (`crates/slot/src/server/mod.rs`).
- Depends on `story:all-sessions-on-slot` (adoption order) and on `story:prometheus-metrics` (last
  before it on `crates/slot/src/server/mod.rs` and `server/tests.rs`, order written out in
  `story:disk-guard`). The edit to `website/docs/agent-integration.md` lands before or after the
  site is deployed (`story:docs-site-live` does not edit that page), so there is no edge to it.

## Acceptance

1. With the shim directory first on `PATH`, `cargo test` in a test repository produces one line in
   `jobs.jsonl` whose job ran the real `cargo` (its resolved program path is outside the shim
   directory), and exits with cargo's exit code.
2. `cargo metadata --format-version 1`, `go env GOPATH` and `go list ./...` through the shim
   produce no job-log line; and with rust-analyzer running on the test repository for 60 s after a
   file is opened and edited once, `jobs.jsonl` gains no line.
3. With the daemon stopped, `cargo --version` through the shim still prints cargo's version (the
   client's local fallback), with the one stderr line saying it ran locally.
4. A test covers that a job whose `PATH` starts with the shim directory does not start the shim.
5. A verification-report covering five consecutive working days after the shim is installed on
   the workstation shows at least 90 % of build-class calls in the session logs present in
   `jobs.jsonl`, with queue wait and run time p50/p90 beside the pilot's before-numbers
   (`story:pilot-session-measured`). This is the measurement `epic:adoption-measured` closes on.

## Scope

- `contrib/shim/` (shim scripts) or `crates/slot-cli/src/main.rs` (if the shim is the `slot` binary
  invoked as `cargo`)
- `crates/slot/src/client.rs`, `crates/slot/src/server/mod.rs` (`look_path`, shim exclusion),
  `crates/slot/src/server/tests.rs`
- `website/docs/agent-integration.md`
