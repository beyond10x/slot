---
format: aep.planning-md/3
id: story:git-dependency-workstation-steps-run-under-a-pool-rule
kind: story
status: draft
title: The workstation steps of a git dependency copy run under a pool rule
revision: 1
---
# The workstation steps of a git dependency copy run under a pool rule

## Why

Before a remote job whose lock file names git sources, the client runs `git ls-files`,
`git ls-remote --get-url`, `ssh -G`, `cargo metadata --locked --offline` and `cargo fetch --locked`
on the workstation, outside the pool, in the checkout's lock directory. The checkout's
`.cargo/config.toml` (a `build.rustc-wrapper`, credential providers) and `rust-toolchain.toml`
apply to the two cargo calls there. `docs/design/remote-pool.md` § "What runs where" records it.

## Acceptance

- The design names, for each of those commands, where it runs and under which pool rule (no slot,
  a slot of the job, or a slot of its own), with the measured time of each on a checkout with git
  dependencies.
- `cargo metadata` and `cargo fetch` either take a slot or the design says why they need none,
  with the measurement.
- A checkout config that sets `build.rustc-wrapper` or a credential provider does not run a
  program of the checkout's choosing on the workstation before the job, or the design names this
  as a limit the deny lists do not cover.
