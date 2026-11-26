---
format: aep.planning-md/3
id: story:a-remote-tree-is-one-repository
kind: story
status: implemented
title: A remote job's tree is one repository's work tree
summary: a job started in a folder of several repositories, or at the same-path root, is refused before any sync
relations:
- serves: vision:O6
scope:
- confidence: cited
  path: CHANGELOG.md
- confidence: cited
  path: crates/slot-cli/tests/remote_sync/safety.rs
- confidence: cited
  path: crates/slot/src/client.rs
- confidence: cited
  path: crates/slot/src/same_path.rs
- confidence: cited
  path: docs/design.md
- confidence: cited
  path: website/docs/remote-builds.md
revision: 10
transitions:
- {from: "draft", to: "proposed", at: "2026-11-25T09:19:34Z", actor: "agent:slot", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-25T09:19:34Z", actor: "agent:slot", revision: 3}
- {from: "active", to: "implemented", at: "2026-11-25T10:12:32Z", actor: "agent:slot", revision: 10, decided_on: {"recorded":{"test_result":1}}}
---
## Outcome

A remote job is synced to a build server only when its tree is one repository's work tree, or a
plain folder that holds no repository. A job started in a folder of several repositories, in a
folder directly under the same-path root, or in the same-path root itself does not run: it ends
with exit 3 before any byte is sent.

## Context

- 2026-11-25, 09:08 CET: a sub-agent ran `cargo nextest run --help` in a folder that holds 87
  repositories and is no git checkout. The cargo shim took the working directory as the tree
  (`git rev-parse --show-toplevel || pwd`), and the client mirrored it at its own path on the
  build server (`client.rs`: a same-path job outside any git checkout mirrors its working
  directory). The rsync ran at 2,000 KiB/s until 10:05:51 CET and created 40 top-level entries
  on the server; a second call from the same folder at 10:11 CET created 2 more.
- The rsync outlived its client (parent pid 1 for about 57 minutes).
- The shim's return sync (`rsync --checksum --update` from the server back into the tree)
  runs after such a job too, so files of the server's copy could come back into the folder.
  Read on the workstation: 1,412 files in 4 repositories got a new change time in 09:08-09:12
  CET; none differs in content from the git index.
- Standing defect beside it: the host's rsync wrapper replaces `--filter=:- .gitignore` with an
  exclude list of cargo target directories, so git-ignored files (`node_modules/`, `.npmrc`,
  build output, `.env` where present) reach every same-path mirror, not only in this incident.
  Read on the server: the older same-path mirrors hold 3 `node_modules` directories and 3
  `.npmrc` files.

## Acceptance

- `remote_sync::safety::a_tree_root_that_is_not_one_repository_is_refused`: the same-path root,
  a folder directly under it, and folders holding repositories at depth 1 and 2 each exit 3
  with `<dir> is not one repository's work tree (<why>)`, and the fake server's log shows no
  rsync. A repository inside such a folder is still synced.
- `same_path::tests::a_folder_that_cannot_be_read_is_refused`: a plain folder passes; a folder
  whose subfolder cannot be read is refused.
- The docs (`website/docs/remote-builds.md`, `docs/design.md`) name the refusal.

## Not in this story

- The cargo shim and the rsync wrapper are host files, not in this repository. Their owner
  gets: the shim's own tree choice (`|| pwd`) and its return sync need the same refusal; the
  wrapper must keep the `.gitignore` filter; an rsync must not outlive its client.
- Removing the server-side copy: a root bundle, applied by the host's operator.
