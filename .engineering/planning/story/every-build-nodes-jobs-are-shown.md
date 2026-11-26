---
format: aep.planning-md/3
id: story:every-build-nodes-jobs-are-shown
kind: story
status: draft
title: slot status and the web view list the jobs of every build node, also one that gets no hand-off
relations:
- serves: vision:O2
revision: 1
---
## Outcome

`slot status` and the web view list the running and queued jobs of every build node a host
sends jobs to, each under its node's name, also a node the daemon never hands a queued job to.

## Why

Seen 2026-11-26 on a host with two build nodes: the host's wrapper sends a job to the node its
repository is mapped to; only one node is a `[remote.<name>]` entry of `cluster.toml`, because
an entry also lets the daemon hand any queued local job to that node, which the per-repository
split must not allow. A consumer's job printed "synced to <second node>", and `slot status` and
the web view listed only one job, on the first node, while the second node ran six. The jobs
were found with `ssh <node> slot state` and `jq`. A node also enters the daemon's list only at a
restart.

## Acceptance

- Red first: a test with a cluster file of two nodes, one of them marked to receive no hand-off
  (`hand_off = false`), and a stub node that answers `slot state` with two running jobs: before
  the change `slot status` lists no job of that node.
- After it `slot status`, `/api/state` and the web view list both nodes' active jobs with the
  node's name and the node's job id; a queued local job is never handed to the node marked
  `hand_off = false`; an entry without the key is handed jobs as before.
- A node added to or removed from the cluster file is shown or dropped at the file's next
  reread, without a restart; its jobs carry `#[serde(default)]` fields only.
- A node that does not answer is shown as not answering, and the other node's jobs are still
  listed.
- `website/docs/remote-builds.md` says how a node that only a wrapper sends jobs to is shown.
