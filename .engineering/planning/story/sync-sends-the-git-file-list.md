---
format: aep.planning-md/3
id: story:sync-sends-the-git-file-list
kind: story
status: draft
title: The sync sends git's file list and the ignored paths the opt-in names, as rsync deltas
relations:
- decomposes: epic:compute-cluster
- serves: vision:O2
- informed_by: executable-system-specification:compute-cluster
- depends_on: story:results-come-back-from-a-node
scope:
- confidence: inferred
  path: crates/slot/src/client.rs
- confidence: inferred
  path: crates/slot/src/toolchain.rs
revision: 4
---
# Story: the sync sends git's file list, and the opt-in's include paths

## Outcome

The files a node receives are the files git lists for the tree (`git ls-files --cached --others
--exclude-standard`), plus the ignored paths the opt-in's `include` names (for example a built UI
that a build script embeds), sent with rsync deltas over the previous sync.

## Specification

`spec/domains/cluster.yaml` Repository `include`, declared by `story:results-come-back-from-a-node`.

## Design

`docs/design/remote-pool.md` § 4.4 (rsync's reading of `.gitignore`; `!keep` lines; global ignore
files; gitignored build inputs), § 4.3, § 4.6.

## Acceptance

1. A tree with a `!keep` line, a file ignored only by `.git/info/exclude` and a tracked file that
   matches an ignore line syncs exactly the files `git ls-files --cached --others
   --exclude-standard` lists, compared by a test.
2. An opt-in with `--include ui/dist` sends `ui/dist` although `.gitignore` excludes it.
3. A one-file change after a full sync transfers that file and the file list only (rsync's
   `--stats` in the test).
4. The deny list of `story:sync-holds-back-secrets` still holds over the git list.
5. Tests: no network; each under 60 s.

## Scope

`crates/slot/src/client.rs`, `crates/slot/src/toolchain.rs`.
