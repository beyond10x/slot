---
format: aep.planning-md/3
id: story:same-path-mirrors-hold-no-git-ignored-files
kind: story
status: draft
title: A same-path mirror holds no git-ignored file of the workstation
summary: the rsync on PATH keeps the .gitignore filter, or the client refuses it; overwritten mirrors are refreshed on the operator's word
relations:
- serves: vision:O6
scope:
- confidence: cited
  path: crates/slot/src/sync.rs
revision: 2
---
## Outcome

A same-path mirror on a build server holds what the client's own sync sends: the tracked files
and the untracked files git does not ignore, plus the server's own build output (`target/`).
No git-ignored file of the workstation (`node_modules/`, `.npmrc`, `.env`, `website/build/`,
`.docusaurus/`) reaches a mirror.

## Context

- The client syncs a tree with `--filter=:- .gitignore` (`sync.rs`). A host-level rsync wrapper
  placed before `/usr/bin/rsync` on `PATH` replaces that filter with an exclude list of cargo
  target directories, so every git-ignored file crosses. This holds for every same-path mirror,
  not only for the incident of `story:a-remote-tree-is-one-repository`.
- Read on the build server on 2026-11-25: the 11 older same-path mirrors hold 3 `node_modules`
  directories and 3 `.npmrc` files; no `.env` was found in them.
- 3 mirrors (named in the incident record, not here) were overwritten on 2026-11-25 by the
  stray sync of a whole folder; they hold the git-ignored files of that folder's copies.
- The client cannot tell from inside whether the rsync on `PATH` keeps its filter. It already
  refuses a wrapper that adds `--copy-unsafe-links` (`an_rsync_that_follows_outside_links_is_refused`).

## Acceptance

- The client refuses an rsync on `PATH` that drops `--filter=:- .gitignore` from its arguments
  (read with a probe sync of a fixture with an ignored file into a temp directory, as the
  unsafe-links check does), or a test shows the wrapper keeps the filter: a fixture with a
  git-ignored `.env` synced through the client's rsync arrives without it.
- A refresh of one mirror, run on the server by its operator, removes every path that
  `git -C <mirror> ls-files --others --ignored --exclude-standard --directory` lists, except
  `target/` and paths under it, and prints each path with its size before removal. Proposed for
  the 3 overwritten mirrors after the wrapper keeps the filter; nothing is removed before the
  operator's word.

## Not in this story

- The rsync wrapper itself is a host file outside this repository; its owner changes it.
