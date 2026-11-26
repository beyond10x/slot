---
format: aep.planning-md/3
id: story:a-compile-inside-a-job-kept-here-goes-to-the-node-or-is-refused
kind: story
status: draft
title: A compile inside a job kept on the host goes to the node or is refused
relations:
- serves: vision:O2
revision: 1
---
# Story: a-compile-inside-a-job-kept-here-goes-to-the-node-or-is-refused

## Outcome

While a build node is on, a cargo, go or rustc call started inside a job that was kept on the host
(reason `cache rule` or `here`) does not compile on the host. The cargo shim no longer passes it
straight to the slot binary with the outer job's reason: it goes through the same routing as a
top-level call, so a compile in a tree under `~/.cache` is refused with exit 3 and the line that
names the managed-worktree move, and a compile in any other tree runs on the node.

## Evidence (2026-11-24, 12:02 CET)

- Job 63586: `cargo nextest run …` in a session scratch copy of a repository, reason `cache rule`,
  ran on the host while the node was on. Jobs 63105, 63109 and 63110 earlier the same day:
  `cargo build --bins` with the same reason.
- The top-level refusal of a compile under `~/.cache` did not apply: the call came from the cargo
  shim, which execs the slot binary directly when `SLOT_LOCAL_REASON` is set.

## Acceptance

- Inside `slot sh -c 'cargo build'` run from a tree under `~/.cache`, cargo exits 3 with the line;
  the outer job ends non-zero.
- Inside a `--here` job in a worktree, `cargo build` runs on the node; `git fetch` stays on the host.
- With no node on, both compile on the host as before.
