---
format: aep.planning-md/3
id: story:a-tree-reaches-a-node-only-with-an-allowed-remote
kind: story
status: active
title: A tree reaches a build node only with an allowed remote, and slot can remove one from a node
relations:
- serves: vision:O2
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-27T18:45:11Z", actor: "human:operator", revision: 3}
- {from: "proposed", to: "active", at: "2026-11-27T18:45:11Z", actor: "human:operator", revision: 4}
---
## Outcome

A tree reaches a build node only when one of its git remotes matches an allow list of the host.
A tree with no remote, no git, or only remotes the list does not name runs in this machine's
pool, with the reason in its record. A tree that reached a node by mistake can be removed there
with one slot command.

## Why

Seen 2026-11-27: a private repository with no remote was synced to a build node, because the deny
lists (paths, and `origin:` patterns of a tree's remotes) can only name what someone thought of in
advance; a tree with no remote matches no `origin:` pattern. The owner added its two paths to the
deny list by hand after the sync. Removing the synced tree from the node needed a root login or a
hand-run job, since `slot shell` admits no delete.

## Design

1. `remote-allow-origins` beside `remote-deny` (`$XDG_CONFIG_HOME/slot/`; `cluster.toml` may name
   another path with `allow_origins_file`): one shell pattern per line, matched against every
   remote URL of the checkout, `#` comments. When the file exists, a job's tree goes to a node only
   when at least one remote URL matches a line and no deny entry matches. Without the file,
   behaviour is as today.
2. The check runs in the client wherever the deny check runs: remote mode with `SLOT_HOST`, a job
   the daemon hands to a node, and every tree a `.slot.toml` `trees` entry declares. A refused
   tree runs here with `local_reason` `never sync: no allowed remote (<tree>)`; a declared tree
   that is refused keeps the whole job here.
3. A tree without git (a plain folder) has no remote and stays here when the file exists.
4. `slot nodes forget <node> <path>`: runs the new read-write step `forget <path>` of `slot shell`
   on the node, which removes a synced tree at its same-path location or a workspace mirror,
   refuses any path outside the same-path root and the workspace root, the root itself, a path
   with `..` or a link in it, and a tree a running job uses. It prints what it removed and the
   bytes freed.

## Acceptance

- Red first: a test with an allow file naming one origin pattern and a repository with no
  remote: before the change the job is synced to a stub node; after it it runs here with the
  reason.
- A repository whose remote matches the allow list and no deny entry is synced as before.
- A declared sibling tree with no allowed remote keeps the job here.
- Without the allow file nothing changes (the existing remote tests pass unchanged).
- `slot nodes forget` removes a mirror on a stub node, refuses `..`, a link, the root and a path
  outside the roots, and refuses a tree a running job uses.
- `website/docs/remote-builds.md` names the allow file and `slot nodes forget`.
