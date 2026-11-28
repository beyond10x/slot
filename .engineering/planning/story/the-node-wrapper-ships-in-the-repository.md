---
format: aep.planning-md/3
id: story:the-node-wrapper-ships-in-the-repository
kind: story
status: draft
title: 'The node mode of the workstation ships in the repository: wrapper, helpers and the sync''s filters'
relations:
- serves: vision:O6
revision: 1
---
## Outcome

Everything the workstation side of node mode runs ships in the repository and its release: the
`slot` wrapper on the agent `PATH`, the `--manifest-path` tree change, the variables sent to a
node, the refusal log and the `rsync` wrapper. An install from a release gives the node mode that
runs on the workstation today, and the sync the repository describes is the sync that runs.

## Why

Goal (2026-11-28): slot deployable from a cloudspec architecture, with weft as the runtime
(`docs/design/cloudspec/README.md`). A deployment installs what the repository ships. Measured
2026-11-28: the wrapper is 370 lines of POSIX sh outside the repository, with four helpers
(`tree-cd.sh` 26, `env-names.sh` 7, `refusal-log.sh` 7, `rsync-wrap/rsync` 106 lines). The
`rsync` wrapper replaces the client's `--filter=:- .gitignore` (`crates/slot/src/sync.rs:644`)
with an exclude list of cargo build directories, so ignored files other than build output reach
a node; a session answered a question on the sync from the repository's code and was wrong for
this host.

## Design

1. Each behaviour of the wrapper and its helpers moves into the `slot` binary, or into
   `contrib/` with a shell test beside it (`contrib/shell-tests.sh`): node route by
   `slot nodes route`, hold files per node, the toolchain parity wait with its bound, the
   refusals (a compile under the cache directory, `--here` for a compile, a script under the
   session scratch directory), the return sync switch, the tree of `--manifest-path`, the
   forwarded variables, the refusal log.
2. The sync's filters are the client's: the cargo build directory list and the `--delete` rule
   for a tree with another running job move into `crates/slot/src/sync.rs`, with tests; the
   upload cap and slots become settings of the client.
3. No private name, host, path or session in what moves: a default node name, a person's rule
   and a time zone in the script's comments are replaced by neutral text.

## Acceptance

- With the old wrapper and helpers removed from the agent `PATH` and the release installed, the
  shell tests and `cargo test` cover each behaviour of design point 1, and a node job from a
  worktree runs, routes and returns as before (one job per refusal and one plain job, exit codes
  read from `slot`).
- `crates/slot/src/sync.rs` sends a git-ignored file that is not a cargo build directory, and
  leaves out `<dir>/target/` beside a `Cargo.toml`; a test shows both.
