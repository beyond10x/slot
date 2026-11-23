---
format: aep.planning-md/3
id: story:cluster-night-measured
kind: story
status: draft
title: One night with two remote nodes meets the epic's measures
relations:
- decomposes: epic:compute-cluster
- serves: vision:O2
- informed_by: executable-system-specification:compute-cluster
- depends_on: story:nodes-start-on-queue-pressure
- depends_on: story:warm-caches-across-node-lives
- depends_on: story:spot-nodes-and-reclaim
- depends_on: story:new-worktree-seeded-from-a-sibling
scope:
- confidence: inferred
  path: docs/design/remote-pool.md
revision: 3
---
# Story: one night with two remote nodes meets the epic's measures

## Outcome

The epic's acceptance is measured: one night with two remote nodes of the operator's templates,
read from the endpoint's job log and the census as `docs/design/remote-pool.md` § 9.5 says.

## Specification

`spec/domains/cluster.yaml` views NodeList and Jobs (the job's `node`, `sync_ms`, `fetch_ms`,
`fell_back`, `rerun_of`); `spec/domains/hostconfig.yaml` Provider (cost cap).

## Design

`docs/design/remote-pool.md` § 9.5 (measures and targets).

## Acceptance

A `verification-report` in this store, from one night of at least 4 hours with two remote nodes,
records each measure of § 9.5 against its target, with the window and the commands that read it:

1. local queue wait p90 during node hours, against under 5 minutes;
2. median and p90 overhead per remote job for warm worktrees, against under 2 s and 5 s;
3. the share of rust CPU seconds run on remote nodes, against at least half;
4. fallbacks, reruns and lost nodes as a share of remote jobs, against under 1 %;
5. remote test results that a local run of the same tree contradicted, against none;
6. the night's cost against the provider's cap.

A measure that misses its target is filed as its own story, named in the report.

## Scope

`docs/design/remote-pool.md` (§ 9.5 gains the measured values).

## Blocked on

`decision-blocker:cluster-night-spend`: the two paid machines for the night are the operator's
spending decision.
