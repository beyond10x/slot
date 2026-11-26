---
format: aep.planning-md/3
id: story:a-denied-git-dependency-keeps-the-job-on-this-machine
kind: story
status: draft
title: A job with a denied git dependency runs on this machine, whichever way it reached slot
relations:
- serves: vision:O2
revision: 1
---
## Outcome

A job whose `Cargo.lock` names a git dependency that may not be synced to a build node runs in
this machine's pool, whichever way the job reached slot, and its record names the reason.

## Why

Reported by a consumer session on 2026-11-26: plain `cargo` in a managed worktree, sent with
`SLOT_HOST` set, ended with exit 3 and `slot: git dependency denied: git+https://…?tag=…: the
tree is not synced to <node>`; the job ran nowhere. The same repository's jobs that reach the
daemon without `SLOT_HOST` run on this machine with `local: git dependency denied: …` in
`slot status`. `--here` in front of a compile is refused by the host, so the session wrapped
its builds in a script to get them to run. That day 13 such jobs used 1.5 CPU-hours here.

## Acceptance

- Red first: a test with `SLOT_HOST` set and a `Cargo.lock` naming a denied git source exits 3
  before the change.
- After it the job runs in this machine's pool and exits with the command's status; stderr has
  one line with the reason; the job record's `local_reason` starts with `git dependency denied`;
  no sync and no job reach the node.
- A job with an allowed git dependency still runs on the node; a failed ssh, a failed sync and a
  node that did not start the job still end with exit 3.
- `website/docs/remote-builds.md` says what happens to a job with a denied git dependency.
