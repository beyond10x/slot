---
format: aep.planning-md/3
id: story:a-compile-outside-a-job-is-refused
kind: story
status: draft
title: A cargo or rustc started outside a slot job is refused
relations:
- serves: vision:O2
- informed_by: story:compiles-outside-the-pool-are-reported-within-a-minute
revision: 1
---
# Story: a-compile-outside-a-job-is-refused

## Outcome

A cargo or rustc started on the host outside a slot job (no `SLOT_JOB`), by any route (an xtask binary calling cargo by its toolchain path, a Taskfile, a script), does not compile: it ends with exit 3 and one line naming the command and how to run it through slot.

## Evidence (2026-11-24, 08:30 CET)

- A `task test-step` ran an xtask binary that started rustup's cargo by path: about 20 rustc outside the pool on a 40-slot host at load 110. The PATH shim never saw it.

## Recommendation

- Every compile passes the rustc wrapper (`build.rustc-wrapper`). A wrapper that checks `SLOT_JOB` (or the compile cache server as caller) and then runs the compile cache client refuses the rest. Editors (rust-analyzer) set a variable that admits them, or run outside the host's cargo config.

## Acceptance

- A test runs `cargo build` by absolute path with no `SLOT_JOB`: exit 3, the line, no rustc process started.
- The same build inside `slot run` compiles through the compile cache as before.
