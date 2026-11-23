---
format: aep.planning-md/3
id: story:placement-sticks-to-a-node
kind: story
status: draft
title: An opted-in worktree is placed on a remote node once, and its jobs run there
relations:
- decomposes: epic:compute-cluster
- serves: vision:O2
- informed_by: executable-system-specification:compute-cluster
- depends_on: story:node-health-poll
- depends_on: story:sync-holds-back-secrets
- depends_on: story:cluster-view-in-web-ui
scope:
- confidence: inferred
  path: crates/slot/src/cli.rs
- confidence: inferred
  path: crates/slot/src/client.rs
- confidence: inferred
  path: crates/slot/src/proto.rs
- confidence: inferred
  path: crates/slot/src/rewrite.rs
- confidence: inferred
  path: crates/slot/src/server/mod.rs
- confidence: inferred
  path: crates/slot/src/server/nodes.rs
- confidence: inferred
  path: crates/slot/src/server/placement.rs
- confidence: inferred
  path: crates/slot/src/types.rs
- confidence: inferred
  path: ui/src/generated/types.ts
- confidence: inferred
  path: website/docs/remote-builds.md
revision: 4
---
# Story: an opted-in worktree is placed on a remote node and stays there

## Outcome

The daemon places a worktree of an opted-in repository on a Ready remote node with room, once,
and every later job of that worktree runs there while the placement lasts. Sessions call `slot
cargo test` as before; the client follows the daemon's placement frame, syncs, runs on the node
and reports back on its open local connection.

## Specification

`spec/domains/cluster.yaml`: Repository, AllowRepository, DisallowRepository; Placement,
PlaceWorktree (not-allowed, local-node, not-ready, lease-too-short, already-placed),
UnplaceWorktree; SubmitJob (placed-elsewhere, too-many-tokens); EndJob (`sync_ms`); views
Placements, PlacementsOnNode, Repositories. The `max_worktrees` bound (`LIMIT:` in the spec) is
enforced here.

## Design

`docs/design/remote-pool.md` § 3.1 (A: the daemon places, the client carries; request flag,
placement frame, end report), § 3.2 (rules 1 to 6, in order), § 3.7 (job-log fields, the
session's lines, the stale-binary line, target paths rewritten), § 6 (built-in local list),
§ 4.4 (the file name of `argv[0]` for a placed job), § 9.5 (`sync_ms`, `fetch_ms` for the
overhead measure).

## Acceptance

1. `slot node allow <repository>` and `disallow` change the Repositories view; a job of a
   repository not allowed runs on the local node.
2. A table-driven test covers each rule of § 3.2 in order, with one row per refusal of
   PlaceWorktree, and a row where the node holds `max_worktrees` placements already.
3. Against a stand-in node, two jobs of one worktree run on the same node; with a second Ready
   node present, a third job of that worktree still runs on the first.
4. A client without the new request flag never gets a placement frame and runs locally.
5. `cargo fmt`, `cargo update`, `cargo run`, `cargo install`, `cargo publish` and node toolchains
   run on the local node whatever the placement (§ 6 list).
6. A placed job whose `argv[0]` is an absolute path runs on the node by its file name.
7. The job record carries `node`, `sync_ms`, `fetch_ms` (when `--fetch` ran) and `remote_id`;
   stderr prints `slot: job N on <node>, synced in X s`, and after a remote `cargo build` without
   `--fetch` the stale-binary line.
8. Output naming the node's target directory reads `target/…`.
9. Tests: no network; each under 60 s, the step under 120 s.

## Scope

`crates/slot/src/server/placement.rs` (new), `crates/slot/src/server/mod.rs`,
`crates/slot/src/server/nodes.rs`, `crates/slot/src/proto.rs`, `crates/slot/src/client.rs`,
`crates/slot/src/types.rs`, `crates/slot/src/cli.rs`, `crates/slot/src/rewrite.rs`,
`ui/src/generated/types.ts` (generated), `website/docs/remote-builds.md`.

Shared with stories of other epics, not ordered against this one:
`website/docs/remote-builds.md` with `story:remote-mode-on-a-real-server`, `client.rs` with
`story:path-shim`. Whichever merges second rebases.

## Blocked on

`decision-blocker:opt-in-repository-key`: what identifies a repository in the opt-in.

## Not in this story

Failure handling (`story:node-leaves-without-stranding-jobs`); the opt-in's file patterns
(`story:results-come-back-from-a-node`).
