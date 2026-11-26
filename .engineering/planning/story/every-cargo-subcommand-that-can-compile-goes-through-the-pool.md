---
format: aep.planning-md/3
id: story:every-cargo-subcommand-that-can-compile-goes-through-the-pool
kind: story
status: draft
title: Every cargo subcommand that can compile goes through the pool
relations:
- serves: vision:O2
revision: 1
---
# Story: every-cargo-subcommand-that-can-compile-goes-through-the-pool

## Outcome

The PATH shim sends every cargo subcommand through slot, including `cargo run`, aliases such as `cargo xtask` and external subcommands, except a fixed list that compiles nothing (`fmt`, `metadata`, `tree`, `update`, `clean`, `add`, … and `mutants`, whose own builds start cargo by path).

## Evidence (2026-11-24)

- `cargo xtask render-bundle` (an alias of `cargo run -p xtask`) compiled outside the pool 4 times in one day; the shim sent only build, check, test, clippy, doc, bench, nextest, install, rustc, llvm-cov and miri.

## Acceptance

- `contrib/slot-shim.test.sh` (in `task check`, under 1 s) routes `cargo run`, `cargo xtask`, `cargo +1.99 run` and `cargo --color never run` to slot, and `fmt`, `metadata`, `tree`, `--version`, `clean`, `mutants`, `update` directly; it fails on the shim before this change (4 cases).
