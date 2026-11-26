---
format: aep.planning-md/3
id: story:an-exported-rustc-wrapper-does-not-pass-the-gate
kind: story
status: draft
title: An exported RUSTC_WRAPPER does not get a compile past the rustc gate
relations:
- serves: vision:O2
- informed_by: story:a-compile-outside-a-job-is-refused
revision: 1
---
# Story: an-exported-rustc-wrapper-does-not-pass-the-gate

## Outcome

A session that exports `RUSTC_WRAPPER` or `CARGO_BUILD_RUSTC_WRAPPER` (for example to `sccache`) still has its compiles checked by the rustc gate: outside a slot job they are refused like any other.

## Evidence (2026-11-24)

- The gate sits in `build.rustc-wrapper` of the user's cargo config. Cargo lets the two environment variables override that setting, so a cargo started with `RUSTC_WRAPPER=sccache` compiles outside the pool without passing the gate.

## Recommendation

- The PATH shim, for every cargo call it runs, replaces a wrapper variable that names sccache with the gate and keeps the original in `SLOT_GATE_NEXT`; the gate runs that one instead of sccache.
- The daemon's census reports a rustc outside any job whose parent is not the gate (story:compiles-outside-the-pool-are-reported-within-a-minute).

## Acceptance

- `RUSTC_WRAPPER=sccache cargo build` outside a slot job, through the shim: exit 3 with the gate's line.
- The same inside a slot job compiles through sccache.
- A cargo started by path with the variable set appears in `slot status` within 60 s.
