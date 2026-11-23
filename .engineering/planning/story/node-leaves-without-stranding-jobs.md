---
format: aep.planning-md/3
id: story:node-leaves-without-stranding-jobs
kind: story
status: draft
title: A failed sync, an unstarted job, a dropped link or a lost node ends with the job run once on the local node
relations:
- decomposes: epic:compute-cluster
- serves: vision:O2
- informed_by: executable-system-specification:compute-cluster
- depends_on: story:placement-sticks-to-a-node
scope:
- confidence: inferred
  path: crates/slot-cli/tests/handover.rs
- confidence: inferred
  path: crates/slot/src/client.rs
- confidence: inferred
  path: crates/slot/src/server/handover.rs
- confidence: inferred
  path: crates/slot/src/server/mod.rs
- confidence: inferred
  path: crates/slot/src/server/nodes.rs
- confidence: inferred
  path: crates/slot/src/server/placement.rs
revision: 4
---
# Story: a node that leaves does not strand its jobs

## Outcome

A failed sync, a job that never starts on its node, a dropped link and a lost node each end with
the job run once on the local node, never with exit 3 and a lost queue place. A node drains at its
deadline minus 30 minutes and leaves when it is drained and idle or past its deadline; its
placements end when it drains, and survive a reload of the endpoint daemon until then.

## Specification

`spec/domains/cluster.yaml`: FallBackJob (the only move of a job between nodes, first enqueue
time kept), SubmitJob with `rerun_of` (Job.first_attempt), EndJob how NodeLost, DrainNode and
MarkNodeGone ending placements, GoneReason Drained and Deadline, JobEnd.

## Design

`docs/design/remote-pool.md` § 3.5 (failure table), § 3.1 (fallback on the open connection),
§ 3.6 (placements in the handover state), § 7.4 (drain at the deadline minus 30 minutes; stop
when drained and idle, or at the deadline).

## Acceptance

1. With the stand-in node's sync failing, a placed job runs on the local node with its first
   enqueue time; the record has `fell_back: true` and stderr one line. After three failed syncs in a
   row the node is Gone with reason Lost.
2. With the stand-in daemon down after the sync, the job runs locally instead of exiting 3.
3. Killing the stand-in mid-job: the client runs the job once on the local node, prints one line,
   and the new record names the first in `rerun_of`; with `SLOT_NO_RESUBMIT=1` it exits 3.
4. The client's ssh options carry `ServerAliveInterval 15` and `ServerAliveCountMax 3`.
5. A node whose deadline is 30 minutes away moves to Draining and its placements end; its
   running jobs finish. A Draining node with no running job becomes Gone with reason Drained;
   a node past its deadline becomes Gone with reason Deadline.
6. A handover test with a placement open shows the same Placements view before and after, and
   the job on it ends normally.
7. Tests: no network; each under 60 s, the step under 120 s.

## Scope

`crates/slot/src/client.rs`, `crates/slot/src/server/mod.rs`,
`crates/slot/src/server/placement.rs`, `crates/slot/src/server/handover.rs`,
`crates/slot/src/server/nodes.rs`, `crates/slot-cli/tests/handover.rs`.

The later stories that edit `client.rs` and `server/mod.rs` (`story:results-come-back-from-a-node`,
`story:sync-sends-the-git-file-list`, `story:new-worktree-seeded-from-a-sibling`) are ordered
after this one through `story:daemon-conforms-to-cluster-spec`.

## Open

How many reruns one job may have (`UNMAPPED:` on `slot.cluster.Job`): this story takes one, as
§ 3.5 says, until the operator decides otherwise.
