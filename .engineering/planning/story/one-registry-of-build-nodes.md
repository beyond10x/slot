---
format: aep.planning-md/3
id: story:one-registry-of-build-nodes
kind: story
status: active
title: 'cluster.toml is the one registry of build nodes: routing, status, web view and usage read it'
relations:
- serves: vision:O2
- informed_by: story:every-build-nodes-jobs-are-shown
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-11-27T11:09:02Z", actor: "human:operator", revision: 3}
- {from: "proposed", to: "active", at: "2026-11-27T11:09:02Z", actor: "human:operator", revision: 4}
---
## Outcome

`cluster.toml` is the one registry of build nodes on a host. Every node a job can run on is a
`[remote.<name>]` entry; the daemon, `slot status`, `/api/state`, `/api/nodes`, the web view and
`slot usage` all read their node list from it, and nothing else names a node.

## Why

Seen 2026-11-27 on a host with two build nodes: a host wrapper in front of `slot` chose a node per
repository from a file of its own (a repository-to-node map and one environment file per node)
and sent jobs there with `SLOT_HOST`. Only one of the two nodes was a `[remote.<name>]` entry, so
the daemon never polled the other: its jobs did not appear in `slot status`, the web view or
`slot usage`, for a whole day. A node also enters the daemon's list only at a restart
(`Remotes::reread` takes new terms of known nodes and ignores added or removed ones), and
`slot usage` reads only this host's job log, so no job a node ran is counted there at all.

## Design

1. Routing in the registry. A `[remote.<name>]` entry gets `repositories = ["<name>", …]`
   (repository names as `slot status` prints them: the directory of the checkout's common git
   directory, without `.git`), and the file gets `default_node = "<name>"`. The spec
   (`spec/domains/hostconfig.yaml`) and the generated types carry both. The existing `spill`
   key keeps its meaning (`spill = false`: the daemon never hands a queued local job to it).
2. `slot nodes` subcommands (plural: `slot node …` stays a Node.js build command), all reading
   or writing `cluster.toml` through `hostconfig`:
   - `slot nodes list` prints each registered node: name, state as the daemon sees it (when the
     daemon answers), spill, repositories, default.
   - `slot nodes route [<dir>]` prints, as shell assignments, the node a job in `<dir>` (default:
     the working directory) goes to: `SLOT_NODE` (the entry name), `SLOT_HOST`, `SLOT_SSH`,
     `SLOT_REMOTE`, `SLOT_NODE_END` (the entry's deadline as epoch seconds; empty without one,
     so an earlier value is cleared), all five exported. The node is the entry whose
     `repositories` names the repository, else `default_node`. Exit 3 with one line naming the
     file when no entry matches, or when the matched name has no `[remote.<name>]` entry. A
     host wrapper sources this output; it names no node of its own.
   - `slot nodes add <name> --ssh <cmd> [--slot-path] [--deadline] [--price-per-hour-cents]
     [--spill true|false] [--repository <name>]… [--default]` writes the entry (a repository
     listed under another node moves; a `repositories` key left empty is dropped); `slot nodes
     remove <name>` takes it out, and the repositories routed to it then go to `default_node`
     (removing the default node clears `default_node`). Both keep the rest of the file as it
     was (comments and order where `toml_edit` keeps them) and refuse a file that does not
     parse. A node slot provisions is registered with `add` and deregistered with `remove`.
3. Reread adds and removes. `Remotes::reread` adds a node new in the file (Joining) and drops a
   node taken out of it, at the next poll after the file changed; no restart.
4. Node jobs in the log. When a poll shows a node's job ended for the first time, the daemon
   appends that job (with `node` set to the node's name and the node's id) to
   `node-jobs.jsonl` beside `jobs.jsonl`. `slot usage` and `/api/usage` read both files, and
   group and filter by node (`--by node`, a `node` column in the web view's usage table).

## Acceptance

- Red first: a test with a cluster file of one node, then a second entry added to the file with
  `spill = false`, and a stub node that answers the `state` step with two running jobs: before
  the change the second node's jobs are not in `/api/state`; after it they are, without a
  restart, and a queued local job is never handed to it.
- Removing the entry drops the node and its jobs from `/api/state` at the next reread.
- `slot nodes route` in a checkout of a repository listed under a node prints that node's
  assignments; in another repository the default node's; with no match it exits 3.
- `slot nodes add` then `slot nodes remove` leaves a file byte-equal to the start.
- A node job that ended is in `node-jobs.jsonl` once, and `slot usage --by node` counts it.
- The web view's node list comes from `/api/nodes`; no node name is written in `ui/src`.
- `website/docs/remote-builds.md` and the CLI reference describe `slot nodes` and the two keys.
