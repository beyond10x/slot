---
format: aep.planning-md/3
id: story:test-execution-bounded-by-pool
kind: story
status: draft
title: Test execution is bounded by the pool
relations:
- decomposes: epic:desktop-headroom
- serves: vision:O2
scope:
- confidence: inferred
  path: crates/slot/src/jobserver.rs
- confidence: inferred
  path: crates/slot/src/server/mod.rs
- confidence: inferred
  path: crates/slot/src/toolchain.rs
- confidence: inferred
  path: docs/design.md
- confidence: inferred
  path: website/docs/reference/environment.md
- confidence: inferred
  path: website/docs/toolchains.md
revision: 5
---
# Story: test-execution-bounded-by-pool

## Outcome

The test phase of a Rust job stays inside the pool, as its compile phase already does: across all
jobs, runnable test threads plus rustc and rustdoc processes never exceed the pool size. Doc-tests
(rustdoc compiling and running each doc example) are covered by the same bound.

## Context

- Every `cargo …` subcommand (build, test, check, clippy, nextest, doc, run) and `rustc` /
  `cargo-*` is matched as toolchain `rust` (`crates/slot/src/toolchain.rs`, `Cargo::matches`) and
  compiles through the shared jobserver; sccache work is attributed through `SLOT_JOB`.
- **The defect this story removes:** once compilation ends, test binaries run with
  `RUST_TEST_THREADS` / `NEXTEST_TEST_THREADS` set to `max(2, free tokens at job start)`
  (`Cargo::env`), without holding those tokens. Two jobs that start on an idle pool both get the
  whole pool as threads, and the test phase oversubscribes the CPU. That behaviour is what is
  replaced, not kept.
- Doc-tests: `cargo test` runs rustdoc per doc example, in parallel by `RUST_TEST_THREADS`
  (`--test-threads`), and rustdoc is not a jobserver client for that phase.
- Decided here: the test phase's threads are backed by tokens the job holds, so the change touches
  token accounting in `crates/slot/src/server/mod.rs` as well as `Cargo::env`. Which way the tokens
  are held (a fixed share for test commands as go has, or tokens drawn when the test phase starts)
  is the implementer's choice, recorded in `docs/design.md` § 3.2.
- First of the stories that edit `crates/slot/src/server/mod.rs`; `story:disk-guard` follows it by
  `depends_on`.

## Acceptance

Measured with a 2-token pool, two concurrent `slot cargo test` jobs on a crate with a
multi-threaded unit-test suite and doc-tests, sampling `/proc` every 0.1 s: the number of runnable
(state `R`) test threads plus rustc and rustdoc processes across both jobs never exceeds 2. The
sampler and its result are a test or a script run by `task check` or recorded as a
verification-report; the same run without this story's change is recorded as the contrast.

## Scope

- `crates/slot/src/toolchain.rs` (`Cargo::env`, `Cargo::tokens`)
- `crates/slot/src/server/mod.rs` (token accounting)
- `crates/slot/src/jobserver.rs` (if test threads draw tokens)
- `docs/design.md` § 3.2 *Test threads*
- `website/docs/toolchains.md`, `website/docs/reference/environment.md` (both document today's
  `RUST_TEST_THREADS` behaviour)
