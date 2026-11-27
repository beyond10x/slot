---
format: aep.planning-md/3
id: story:a-remote-job-takes-its-declared-sibling-trees
kind: story
status: active
title: A remote job takes the sibling trees its .slot.toml declares
relations:
- serves: vision:O6
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-27T11:09:02Z", actor: "human:operator", revision: 3}
- {from: "proposed", to: "active", at: "2026-11-27T11:09:02Z", actor: "human:operator", revision: 4}
---
## Outcome

A repository whose gate reads a sibling checkout (a specification repository at `../../specs`, a
crate at `../driver`) runs that gate on a build node. Its `.slot.toml` declares the extra trees;
the client syncs each to its own path on the node before the job, under the same deny rules as
the job's own tree.

## Why

Seen 2026-11-27: the most local slot-hours of jobs a rule kept on the host came from
`slot --here task check` in repositories whose gate reads a sibling checkout, and from a
repository whose build runs `cargo build --manifest-path ../<sibling>/Cargo.toml`. A remote job
gets only its own work tree, its git directories and its working directory
(`same_path_extras` in `client.rs`), so these gates fail on a node and their owners keep them on
the host with `--here`.

## Design

1. `.slot.toml` gets `trees = ["../../specs", "../driver"]`: paths relative to the repository
   top (absolute allowed), each resolved and canonicalised by the client.
2. In same-path remote mode the client syncs each declared tree as a `same_path::Extra` of kind
   `Tree` (with its git directories as `Git` extras, so `git` reads work there) before the job,
   with the same bounds as the job's tree.
3. Every declared tree passes the checks the job's own tree passes: `same_path::tree_refusal`,
   under the home, the never-sync list (paths and `origin:` patterns of the tree's own
   remotes). A declared tree that is refused keeps the job on this host with the refusal as its
   `local_reason`; it is never synced.
4. A declared tree that does not exist is a refusal of the job by name (exit 3 in remote mode
   with the path), never a silent skip.
5. `slot status` and the job record name the extra trees synced (`extra_trees`, a
   `#[serde(default)]` field).

## Acceptance

- Red first: a test with a stub remote and a repository whose `.slot.toml` names a sibling
  tree: before the change the sibling is not synced; after it it is synced to its own path.
- A declared tree on the never-sync list keeps the job local with that reason.
- A missing declared tree ends the remote job with exit 3 and its path in the line.
- `website/docs/remote-builds.md` and the `.slot.toml` reference describe `trees`.
