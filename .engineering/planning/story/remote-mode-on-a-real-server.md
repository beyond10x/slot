---
format: aep.planning-md/3
id: story:remote-mode-on-a-real-server
kind: story
status: draft
title: Remote mode verified against a real second machine
relations:
- serves: vision:O2
- informed_by: specification:delivered-baseline
scope:
- confidence: inferred
  path: website/docs/remote-builds.md
revision: 3
---
# Story: remote-mode-on-a-real-server

## Outcome

Remote mode is verified against a real second Linux machine over real ssh, not the local
stand-in: with `SLOT_HOST` set on the workstation, `slot cargo test -p <pkg>` syncs the
repository, runs on the server's pool with the target directory on the server, streams output with
paths rewritten to the client, passes the exit code through, and falls back to a local run with one
stderr line when the server is unreachable. Overhead per warm call is measured against
`docs/design.md` G4 (under 1 s at median).

## Context

- Verified so far only "via a local stand-in for ssh (`rsync -e`, `slot proxy`)"; "real ssh to a
  second machine" is listed as not verified (`docs/design.md` § 9).
- No build server has been chosen: `docs/design.md` § 5 item 4, "Phase 0 measures core-hours before
  any hardware is chosen". Recorded as `decision-blocker:remote-build-host`.
- The ssh forced command (`command="slot-shell",restrict`) is not built (§ 10) and is not part of
  this story.

## Acceptance

A verification-report in this store, from runs against the chosen server, records:

1. a job from the workstation present in the server's `jobs.jsonl` with `remote: true`, its
   `cwd` under the server's workspace root and the target directory under its cache root;
2. a deliberately failing test whose non-zero exit code is the client's exit code;
3. a compiler error whose `file:line` names the client's worktree path;
4. with the server's ssh port blocked, the command running locally and printing one stderr line
   saying so, within 3 s;
5. median and p90 overhead over 20 warm `cargo test` calls (remote wall time minus server run
   time), with the median under 1 s (G4). A median of 1 s or more does not satisfy this story; it
   is recorded and filed as its own story.

## Scope

- No source change expected; defects found become their own stories.
- `website/docs/` (remote mode page) if the real setup needs steps the docs lack.
