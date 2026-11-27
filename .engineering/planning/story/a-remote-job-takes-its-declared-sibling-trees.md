---
format: aep.planning-md/3
id: story:a-remote-job-takes-its-declared-sibling-trees
kind: story
status: active
title: A remote job takes the sibling trees its .slot.toml declares
relations:
- serves: vision:O6
revision: 5
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

## Follow-up

Seen after the merge: a consumer's gate reads its sibling at `SPECS_DIR` (default
`../../specs`). In a managed worktree elsewhere on disk `../../specs` resolves to nothing, and
the consumer sets `SPECS_DIR` to the checkout; `trees = ["../../specs"]` then ended the remote job
with exit 3.

- A `trees` entry may use `${NAME}`, `${NAME:-default}` (default when unset or empty) and a
  leading `~/` (the client's home); nothing else is expanded. A relative result is relative to
  the repository top. Consumer line: `trees = ["${SPECS_DIR:-../../specs}"]`.
- The variables are those the job takes to the node (its toolchain's and `SLOT_ENV`'s,
  `client::node_vars`), so the tree synced is the one the job reads there.
- An unset `${NAME}` without a default is a `Missing` problem naming the variable; a missing
  tree's line names its entry beside the path.
- Tests: `extra_trees::tests::an_entry_follows_the_variable_its_default_and_the_home`,
  `only_braced_variables_and_a_leading_tilde_are_expanded`, and
  `remote_sync::extra_trees::a_declared_tree_follows_the_variable_the_job_carries`.
