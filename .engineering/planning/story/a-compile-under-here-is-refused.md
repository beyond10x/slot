---
format: aep.planning-md/3
id: story:a-compile-under-here-is-refused
kind: story
status: draft
title: A compile under --here is refused
relations:
- serves: vision:O2
revision: 1
---
# Story: a-compile-under-here-is-refused

## Outcome

`slot --here` keeps a step on the host because it needs the host's credentials. A `--here` job whose command is cargo, rustc or go is refused with exit 3 and the line `a compile does not need this host's credentials; drop --here`.

## Evidence (2026-11-24, 08:30 CET)

- A `--here` job of 4 slots compiled a Rust room on the host while a build node was on.

## Acceptance

- `slot --here run -- cargo build` and `slot --here run -- go test ./...` exit 3 with the line; `slot --here run -- git fetch` runs on the host.
