---
format: aep.planning-md/3
id: story:rust-tests-are-killed-at-60-s
kind: story
status: draft
title: The gate's Rust test step kills a test at 60 s
summary: cargo-nextest with a 60 s terminate-after and the ten slowest tests, locally and in CI
relations:
- serves: vision:O2
revision: 1
---
## Outcome

The gate's Rust test step kills a test that runs longer than 60 s and counts it as failed, and the
step prints its ten slowest tests, locally and in CI. The step fails when it runs over 120 s.

## Why

On 2026-11-23 the gate of a pull request ran 35 minutes in CI and was cancelled at the job's
timeout, while the same commit's tests passed locally in 116 s and the rerun passed in 5 min 27 s.
The job log was not kept, so the hanging test is unknown. The gate runs `cargo test --workspace
--locked` (`Taskfile.yml`), which has no per-test bound: one test that waits for ever holds the
runner until the job's timeout. The workspace rule for tests asks for a runner that kills a test
at 60 s as a repository's first budget story.

## Acceptance

- The test step runs under cargo-nextest with `slow-timeout = { period = "30s", terminate-after = 2 }`
  in `.config/nextest.toml`, locally and in CI, and prints the ten slowest tests.
- A test that sleeps 90 s, added on a branch, fails the step within 70 s and is named in its
  output; the branch is not merged.
- The step's wall time is printed, and the step fails over 120 s.
- One integration-test binary per crate, as the workspace rule asks; the count before and after is
  in the pull request.
