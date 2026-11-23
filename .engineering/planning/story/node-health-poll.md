---
format: aep.planning-md/3
id: story:node-health-poll
kind: story
status: draft
title: The daemon polls each remote node, admits it on version and clock, and marks it gone after three misses
relations:
- decomposes: epic:compute-cluster
- serves: vision:O2
- informed_by: executable-system-specification:compute-cluster
- depends_on: story:daemon-knows-nodes
scope:
- confidence: inferred
  path: crates/slot/src/server/mod.rs
- confidence: inferred
  path: crates/slot/src/server/nodes.rs
- confidence: inferred
  path: crates/slot/src/server/poll.rs
- confidence: inferred
  path: crates/slot/src/shell.rs
revision: 5
---
# Story: the daemon polls each remote node, admits it, and marks it gone

## Outcome

Every 5 s the daemon asks each remote node for its state over the node's restricted key. A
joining node with the expected slot version and a clock within 1 s becomes Ready; another version
or clock is refused at join. A node that misses three polls in a row is Gone. The node's job-log
lines arrive in the endpoint's state directory as they are written.

## Specification

`spec/domains/cluster.yaml`: AdmitNode (other-version, clock-off, admitted), RecordNodeState
(exceeds-capacity, gone; the load fields), MarkNodeGone with reason Lost, GoneReason. Ending placements on
MarkNodeGone is `story:node-leaves-without-stranding-jobs`.

## Design

`docs/design/remote-pool.md` § 3.4 (the `state` step of `slot shell`, poll, log copy, three
misses), § 7.4 (join), § 8.2 (restricted key, `slot shell` forced command), § 4.5 (clock).

## Acceptance

1. `slot shell` admits a `state` step and still refuses every other command; a test sends each.
2. Against a stand-in node (the local ssh stand-in with a second daemon), a node added in Joining
   is Ready within 10 s when the versions match, and Gone with reason RefusedAtJoin when the
   stand-in reports another version or a clock 2 s off.
3. Killing the stand-in daemon makes the node Gone with reason Lost within 20 s.
4. A job the stand-in ran appears in `<state-dir>/nodes/<name>/jobs.jsonl` within one poll, and
   `slots_in_use`, running and queued counts, `cpu_busy_milli`, `memory_used_mib` and
   `disk_free_gib` of `GET /api/nodes/<name>` follow the stand-in.
5. A report of more slots in use than the node has is not taken and logs one line.
6. The fields the daemon sends do not change (they exist from `story:daemon-knows-nodes`), so
   `ui/src/generated/types.ts` is unchanged by this story.
7. Tests: no network; each under 60 s, the step under 120 s.

## Scope

`crates/slot/src/shell.rs`, `crates/slot/src/server/poll.rs` (new),
`crates/slot/src/server/mod.rs`, `crates/slot/src/server/nodes.rs`.

## Not in this story

Placement and jobs on remote nodes (`story:placement-sticks-to-a-node`).
