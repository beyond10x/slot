---
format: aep.planning-md/3
id: story:shim-routes-cargo-xtask
kind: story
status: draft
title: The PATH shim sends cargo xtask through slot
summary: cargo aliases that compile, xtask first, go through the pool
relations:
- serves: vision:O2
revision: 1
---
## Outcome

The PATH shim sends `cargo xtask …` through slot, like `cargo build` and `cargo test`. Other
cargo aliases that compile are routed the same way when the repository's `.cargo/config.toml`
defines them as `run`, `build` or `test` commands.

## Why

On 2026-11-23 a repository whose gate runs `cargo xtask` ran 2 builds outside the pool: the shim
(written by `hook.rs`) lists the subcommands it routes, and
`xtask` is an alias for `cargo run --package xtask`, which compiles the xtask crate and then
whatever it starts. `run` is routed directly on purpose (a long-running program would hold a
slot), so the alias goes with it.

## Acceptance

- A shim test: `cargo xtask check` in a repository whose `.cargo/config.toml` has
  `xtask = "run --package xtask --"` goes through `slot`; `cargo run` still runs directly.
- An alias that does not expand to `build`, `check`, `test`, `clippy` or `run --package xtask`
  runs directly as today.
- The hook's refusal text names `cargo xtask` when it refuses one outside the pool.
