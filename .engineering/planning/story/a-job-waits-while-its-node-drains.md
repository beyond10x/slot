---
format: aep.planning-md/3
id: story:a-job-waits-while-its-node-drains
kind: story
status: active
title: A job sent to a node whose daemon drains or restarts waits and then runs there
relations:
- serves: vision:O6
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-27T16:22:50Z", actor: "human:operator", revision: 3}
- {from: "proposed", to: "active", at: "2026-11-27T16:22:51Z", actor: "human:operator", revision: 4}
---
## Outcome

A job sent to a build node whose daemon is draining or restarting waits for it and then runs
there. It never ends with exit 3 because the node's socket is gone for a moment, and it never
runs on the client's machine instead.

## Why

Seen 2026-11-27: a provisioning apply on a node restarted the node's daemon with a drain (the
daemon stops listening and waits for its running jobs, up to the drain bound). For the two
minutes of the drain every job sent to the node ended at once with `slot shell: connect
<runtime dir>/slot.sock: No such file or directory (os error 2)` and `slot: connection to daemon
lost`, exit 3; two consumers saw gates fail and asked whether the node was down. The host's
owner held the node's jobs by hand (a hold file of the host wrapper) until the daemon was back.
A drain may last up to its bound (30 minutes by default).

## Design

1. The node's `slot shell` tells "the daemon's socket is missing or refuses" apart from other
   failures: it prints one line with a fixed prefix (`slot shell: daemon not answering:`) and
   exits with a code of its own.
2. The client, on that answer before the job started, waits and tries again every 5 s, up to
   `SLOT_NODE_WAIT` (default 30 minutes, the drain bound), printing once `slot: waiting for
   <node>: its daemon is not answering (draining or restarting)`. After the bound it ends with
   exit 3 and that line. A job that had started is never sent again.
3. While it waits, the job is listed by `slot status` on the client's machine as waiting for the
   node, as a held job is.
4. The tree is synced again before the retry only when the first sync is older than the wait
   (the node may have restarted and evicted nothing; the sync is cheap when nothing changed).

## Acceptance

- Red first: an integration test with a stub node whose `slot shell` reports the daemon not
  answering twice and then runs the job: before the change the client exits 3 on the first
  answer; after it the job runs and exits with the job's code.
- A stub that never answers ends the client after `SLOT_NODE_WAIT` (set to 2 s in the test) with
  exit 3 and the waiting line.
- A job that started on the node and lost its connection is not sent again.
- `website/docs/remote-builds.md` names the wait and `SLOT_NODE_WAIT`.
