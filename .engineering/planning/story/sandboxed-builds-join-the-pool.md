---
format: aep.planning-md/3
id: story:sandboxed-builds-join-the-pool
kind: story
status: draft
title: Builds in a sandbox queue in the pool and run in the sandbox
relations:
- decomposes: epic:adoption-measured
- serves: vision:O6
revision: 1
---
# Story: sandboxed-builds-join-the-pool

## Outcome

An agent session in a sandbox that refuses `socket(AF_UNIX)` and has no network builds through
the machine's one slot pool: its builds wait for slots, draw parallelism from the same jobserver
and show in `slot status`, while every process of the build runs in the sandbox.

## Context

- Agent sandboxes (bubblewrap with user, pid and network namespaces and a seccomp socket filter)
  allow FIFOs in directories bound in at their host path, and nothing else that reaches the host.
- Running the build through the daemon would run it on the host, outside the sandbox: refused.
- Design: `docs/design.md` § 19.

## Acceptance

1. With `slot gate --dir <dir>` running, `SLOT_GATE=<dir> slot sh -c '…'` prints a
   `MAKEFLAGS` naming `fifo:<dir>/jobserver.fifo` and a `SLOT_JOB`, runs as the client's child,
   exits with the command's code, and the job log books the client's command
   (`crates/slot-cli/tests/gate.rs`).
2. A gated job holds its slots until the client ends; a killed client frees them (same tests).
3. Without a gate the command runs outside the pool with one stderr line (same tests).
4. In a bubblewrap sandbox with user, pid and network namespaces and only the gate directory bound
   in, a gated job shows as running with its slots in `slot status` of the host's daemon.

## Scope

- slot only. The sandbox's settings (`SLOT_GATE`, `PATH` with the shim) belong to whoever starts
  the sandbox.
