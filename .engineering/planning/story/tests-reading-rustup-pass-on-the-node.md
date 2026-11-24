---
format: aep.planning-md/3
id: story:tests-reading-rustup-pass-on-the-node
kind: story
status: active
title: Tests that read ~/.rustup pass on the node as on the workstation
relations:
- serves: vision:O6
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-11-24T15:37:54Z", actor: "agent:slot", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-24T15:37:54Z", actor: "agent:slot", revision: 3}
---
# Tests that read ~/.rustup pass on the node as on the workstation

## Why

On 2026-11-24 a session reported 7 tests red on the build node and green on the workstation, the known symptom of `~/.rustup` being a symlink on the node: a test that resolves or compares the toolchain path sees another path.

## Acceptance

- On the node `~/.rustup` is a directory at the same path as on the workstation, or every path a job can read from it resolves to the same string as on the workstation.
- The parity check compares the resolved path of the active toolchain, not only its digest.
- Red first: a test that compares `rustc --print sysroot` with `$HOME/.rustup/toolchains/<name>` passes on both machines.
