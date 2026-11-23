---
format: aep.planning-md/3
id: story:results-come-back-from-a-node
kind: story
status: draft
title: A changed Cargo.lock and files under return patterns come back from the node
relations:
- decomposes: epic:compute-cluster
- serves: vision:O2
- informed_by: executable-system-specification:compute-cluster
- depends_on: story:placement-sticks-to-a-node
- depends_on: story:daemon-conforms-to-cluster-spec
- depends_on: story:provider-contract-and-node-up-down
scope:
- confidence: inferred
  path: crates/slot/src/cli.rs
- confidence: inferred
  path: crates/slot/src/client.rs
- confidence: inferred
  path: crates/slot/src/rewrite.rs
- confidence: inferred
  path: crates/slot/src/server/mod.rs
- confidence: inferred
  path: crates/slot/src/server/nodes.rs
- confidence: inferred
  path: spec/domains/cluster.yaml
revision: 4
---
# Story: the opt-in names files, and those files come back from the node

## Outcome

The operator's opt-in of a repository carries its file patterns: `fetch` (files every successful
`cargo build` of the repository brings back), `return` (files a job created that come back
whatever the exit code) and `include` (ignored paths the sync sends). A changed `Cargo.lock`, the
`return` files and the `fetch` files come back to the session's tree; remote output carries no
control sequence but colour.

## Specification

`spec/domains/cluster.yaml` Repository and AllowRepository gain `fetch`, `return` and `include`
(lists of patterns) in this story; EndJob.

## Design

`docs/design/remote-pool.md` § 5.1 (fetched files per repository), § 5.3 (`return` patterns),
§ 5.5 (`Cargo.lock` on the side), § 4.4 (ignored paths to include), § 8.6 (each name checked:
relative, no `..`, matching a configured pattern, never under `.git`, `.cargo`, `.github`, never
`.slot.toml` or a build script; control sequences dropped).

## Acceptance

1. `slot node allow <repository> --fetch <pattern> --return <pattern> --include <path>` (each
   repeatable) stores the patterns; the Repositories view and `slot node list --repositories`
   show them; the spec declares them and `ess specify validate --path spec` passes.
2. With `--fetch target/debug/mytool`, a successful placed `cargo build -p mytool` leaves the
   session's `target/debug/mytool` byte-equal to the node's; a failed build brings nothing back.
3. A placed job that adds a dependency changes `Cargo.lock` on the node; after it, the session's
   `Cargo.lock` equals the node's and stderr names it.
4. A failing test run that writes `target/nextest/junit.xml` under a `return` pattern brings it
   back although the exit code is non-zero.
5. A returned name outside the patterns, with `..`, or under `.git` is refused with one line.
6. Output holding a cursor-movement sequence arrives without it; colour sequences stay.
7. The conformance run stays green.
8. Tests use the stand-in node; no network; each under 60 s.

## Scope

`crates/slot/src/client.rs`, `crates/slot/src/server/mod.rs`, `crates/slot/src/server/nodes.rs`,
`crates/slot/src/cli.rs`, `crates/slot/src/rewrite.rs`, `spec/domains/cluster.yaml`.

## Not in this story

Sending `include` paths and git's file list (`story:sync-sends-the-git-file-list`, which depends
on this one).
