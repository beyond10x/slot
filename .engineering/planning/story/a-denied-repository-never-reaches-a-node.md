---
format: aep.planning-md/3
id: story:a-denied-repository-never-reaches-a-node
kind: story
status: active
title: A denied repository never reaches a node, whichever checkout a job runs in
relations:
- serves: vision:O2
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-11-24T15:10:55Z", actor: "agent:slot", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-24T15:10:55Z", actor: "agent:slot", revision: 3}
---
# A denied repository never reaches a node, whichever checkout a job runs in

## Why

On 2026-11-24 a build node held copies of five trees its operator had denied: a linked worktree
of a denied repository, and through it the repository's own git directory, which lives in the
denied primary checkout. The deny list held path prefixes, and the check compared only the job's
working directory with them; the sync of a linked worktree also copies its git common directory.
A preset `SLOT_HOST` skipped the check entirely. The host-side wrapper was fixed the same day;
the binary must hold the rule itself.

## Acceptance

- `slot` reads a deny list (`~/.config/slot/remote-deny`, one entry per line) before any sync to
  a node, and runs the job in the local pool with the reason when an entry matches:
  - a path entry matches the working directory, the checkout's top, its git directory or its git
    common directory;
  - an `origin:<pattern>` entry (shell pattern) matches any remote URL of the checkout.
- No environment variable (`SLOT_HOST`, a cluster file, `--node`) skips the check; only `--local`
  or `--here` choose the local pool without it.
- Every sync source the client sends (tree, git directories, `--fetch` paths, same-path working
  directory) is checked, not just the working directory.
- The refusal is written to the job log with the matching entry.
- Red first: a linked worktree whose primary checkout is denied, and a clone whose remote matches
  an `origin:` entry, both start a sync in a test before the change and none after.
