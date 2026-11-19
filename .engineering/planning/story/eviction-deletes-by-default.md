---
format: aep.planning-md/3
id: story:eviction-deletes-by-default
kind: story
status: active
title: Eviction deletes by default; evict.json sets the dry run
summary: Target-directory eviction deletes by default; a state-dir file turns the dry run on without a unit edit
refs:
- provider: github
  reference: beyond10x/slot#35
relations:
- decomposes: epic:desktop-headroom
- serves: vision:O2
scope:
- confidence: cited
  path: crates/slot/src/cli.rs
- confidence: cited
  path: crates/slot/src/evict.rs
- confidence: cited
  path: crates/slot/src/server/mod.rs
- confidence: cited
  path: crates/slot/src/status.rs
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-19T13:45:50Z", actor: "human:unknown", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-19T13:45:50Z", actor: "human:unknown", revision: 3}
---
# Story: eviction-deletes-by-default

## Outcome

A host whose worktrees each carry a Cargo target directory gets disk back at the eviction floor
without anyone editing the service unit; a dry run under the floor says that eviction is off and
what it would free.

## Context

- Issue #35: the daemon shipped eviction in dry run (`--evict-dry-run` default true), so a full
  disk filled while the journal listed what it would delete.
- Design: `docs/design.md` § 18.

## Acceptance

1. `slot serve` deletes by default; `--evict-dry-run true` is still a dry run
   (`cli::tests::eviction_deletes_by_default`).
2. `<state-dir>/evict.json` (`{"dry_run": …}`) wins over the flag, read every pass; a missing
   file, a missing field or an unreadable file leaves the flag
   (`evict::tests::the_dry_run_follows_the_state_dir_file_over_the_flag`).
3. A dry run under the floor says `eviction is off (dry run), would free <n> GB in <k> dir(s)` in
   the journal and in `slot status`
   (`evict::tests::a_dry_run_under_the_floor_says_eviction_is_off_and_what_it_would_free`,
   `status::tests::a_dry_run_under_the_floor_says_eviction_is_off`).

## Scope

- slot only: `crates/slot/src/{cli,evict,status}.rs`, `server/mod.rs`, docs.
