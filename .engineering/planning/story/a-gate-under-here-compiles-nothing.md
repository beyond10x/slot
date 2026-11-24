---
format: aep.planning-md/3
id: story:a-gate-under-here-compiles-nothing
kind: story
status: draft
title: A gate under --here compiles nothing on the host
relations:
- serves: vision:O2
revision: 1
---
# Story: a-gate-under-here-compiles-nothing

## Outcome

`story:a-compile-under-here-is-refused` refuses a `--here` job whose own command is cargo, rustc or go.
A `--here` job whose command is a gate (`task check`, `make`, `just`, a script) still compiles on the
host through the cargo it starts: the job is a slot job, so the rustc gate lets its compiles through.
While a build node is on, a rustc started inside a `--here` job is refused with exit 3 and the line
`a compile does not need this host's credentials; run the gate without --here`.

## Evidence (2026-11-24, 11:29 CET)

- Job 63491: `slot --here task check` in a worktree of a Rust repository, 1 slot, ran on the host
  while the node was on. The `--here` refusal of the wrapper did not match: its command is `task`.

## Acceptance

- `slot --here run -- sh -c 'cargo build'` and `slot --here task check` in a Rust tree exit 3 with
  the line while a node is on; `slot --here run -- git fetch` runs on the host.
- With no node on, both run on the host as before.
