---
format: aep.planning-md/3
id: story:jobs-run-with-the-callers-open-file-limit
kind: story
status: draft
title: A job runs with the open-file limit of the process that called slot
summary: a command run through slot gets the caller's open-file limit, not the daemon's
relations:
- serves: vision:O6
revision: 1
---
## Outcome

A command run through `slot` gets the open-file limit of the process that called `slot`, not
the limit the daemon was started with.

## Context

- Measured on 2026-11-22 on a workstation whose daemon runs as a systemd user service: the
  daemon's limit is 1,024 soft and 1,000,000 hard (`/proc/<pid>/limits`), a login shell's is
  1,000,000 soft, and `slot sh -c 'ulimit -Sn'` prints 1024.
- A job is a child of the daemon, so it inherits the daemon's limits.
- Effect seen the same day: the test suite of another repository, whose fixture nests 2,200
  directories, fails 3 tests under `slot` with "Too many open files (os error 24)" and passes
  all of them in a shell with the limit raised. Its forge run does not go through slot, so the
  local gate and the forge disagree.
- Not compared: the other limits (stack size, core size, processes).

## Acceptance

1. `slot sh -c 'ulimit -Sn'` prints the caller's soft limit. Where that is above the daemon's
   hard limit, the job gets the daemon's hard limit and one `slot: warning:` line says so.
2. The client sends its limit with the request. The field carries `#[serde(default)]`: an
   older daemon ignores it, and a newer daemon runs a request without it as today.
3. In remote mode the build server's limit applies, and the reference page says so.
4. An integration test runs a job from a client whose soft limit differs from the daemon's and
   reads the limit inside the job.
5. The stack size limit is either passed the same way or named as not covered on the
   reference page.
6. No `unsafe` code is added outside `server/handover.rs`.

## Scope

- `crates/slot/src/client.rs`, `crates/slot/src/server/mod.rs`, `crates/slot/src/types.rs`
- `website/docs/reference/environment.md`
