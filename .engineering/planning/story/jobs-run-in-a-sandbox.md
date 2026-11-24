---
format: aep.planning-md/3
id: story:jobs-run-in-a-sandbox
kind: story
status: draft
title: Every job runs in a sandbox that sees the machine read-only except what it builds
summary: 'slot serve --sandbox runs each job in bubblewrap: read-only filesystem, writable checkout, target, tmp and build caches'
relations:
- serves: vision:O2
scope:
- confidence: cited
  path: CHANGELOG.md
- confidence: cited
  path: crates/slot-cli/src/main.rs
- confidence: cited
  path: crates/slot/src/cli.rs
- confidence: cited
  path: crates/slot/src/lib.rs
- confidence: cited
  path: crates/slot/src/sandbox.rs
- confidence: cited
  path: crates/slot/src/server/mod.rs
- confidence: cited
  path: crates/slot/src/server/tests.rs
- confidence: cited
  path: docs/design.md
- confidence: cited
  path: docs/design/remote-pool.md
- confidence: cited
  path: website/docs/reference/cli.md
- confidence: cited
  path: website/docs/reference/environment.md
- confidence: cited
  path: website/docs/remote-builds.md
revision: 4
---
# Story: jobs run in a sandbox

## Acceptance

1. `slot serve --sandbox` runs every job inside bubblewrap (`bwrap`): the whole filesystem is
   bound read-only (the home directory, `~/.rustup`, `~/.cargo/bin`, `/usr`, `/usr/local`), in
   its own pid and IPC namespace.
2. Writable inside a job: its checkout (a synced workspace `<ws-root>/<id>`, or the git
   checkout of a local job's directory), its target directory, a private `/tmp` (`TMPDIR=/tmp`),
   `registry/` and `git/` of `CARGO_HOME`, the Go caches when they exist, and each
   `--sandbox-write` path. `docs/design.md` names each with its reason.
3. Hidden from a job: other synced workspaces, other jobs' target directories under the cache
   root, and the user's runtime directory (the daemon's socket, the user bus).
4. A job under the sandbox cannot write `~/.cargo/bin/x` and cannot delete a file in another
   workspace; it can write its target directory and `/tmp` (daemon tests, each under 60 s).
5. `cargo build` of a tiny crate succeeds under the sandbox, with `HOME` and `CARGO_HOME` in
   temporary directories (daemon test under 60 s).
6. The jobserver FIFO and the job's output pipes work inside the sandbox; the compile cache
   server stays reachable over TCP (the network namespace is shared).
7. With `--sandbox` and no working `bwrap`, `slot serve` refuses to start with a message that
   names the cause and the package to install; it never runs a job unsandboxed.
8. Tests skip with a printed reason where `bwrap` is missing or cannot create namespaces.
9. Docs: a `docs/design.md` section, a website page for the option, the line for the node boot
   script (`dnf install -y bubblewrap`), `CHANGELOG.md` under Unreleased, the CLI reference
   regenerated.
10. With `--sandbox-fixed-paths` (off by default) a job sees its checkout at `/work/src` and its
    target directory at `/work/target`: two worktrees of one repository build at the same paths
    (working directory, `CARGO_MANIFEST_DIR`, `CARGO_TARGET_DIR`; daemon test under 60 s). The job
    runs without a rustc wrapper, and the design says why.

## Outcome

A build cannot delete, replace or install a binary or tool that another build needs, and cannot
change another build's tree or target directory: every job sees the machine read-only except
for what it builds.

## Context

The operator asked whether builds run isolated from each other ("Can one of the Rust builds
delete a binary on the host or replace it or install a tool or remove a tool another build would
need? It needs to be isolated."). Before this story every job ran as the daemon's user with write
access to the whole home directory: toolchains, other workspaces, the compile cache. It starts
with jobs on a build node, where every job is a synced workspace.

## Not covered

- The network: shared, as before (cargo may fetch; the compile cache server is on TCP).
- The workstation's own sessions, which run outside slot; and jobs whose build writes outside the
  writable paths (package-manager stores, browser downloads) until named with
  `--sandbox-write`.
- A writable cache (cargo registry, Go caches) can still be poisoned by one job for the next.
