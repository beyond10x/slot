---
format: aep.planning-md/3
id: story:one-hand-started-node-measured
kind: story
status: draft
title: One hand-started remote node builds chosen commands, and the design's estimates are measured
relations:
- decomposes: epic:compute-cluster
- serves: vision:O2
- informed_by: executable-system-specification:compute-cluster
- depends_on: story:cluster-design-and-spec-in-the-gate
- depends_on: story:sync-holds-back-secrets
scope:
- confidence: inferred
  path: docs/design/remote-pool.md
- confidence: inferred
  path: website/docs/cluster/hand-started-node.md
- confidence: inferred
  path: website/sidebars.ts
revision: 4
---
# Story: one hand-started remote node, measured

## Outcome

One cloud machine, started by hand by the operator for 4 hours, builds chosen commands of two
to four repositories through today's remote mode (`SLOT_HOST` in a wrapper the operator keeps
outside every repository), and the numbers the design only estimates are measured.

## Specification

`slot.cluster.Node` (kind Remote, deadline), `slot.provisioning` (self-termination before use).
No daemon code: this story uses remote mode as built (§ 2.1).

## Design

`docs/design/remote-pool.md` § 9.1 (runbook, steps 1 to 8; step 1 reads the file list the sync
sends; the deny list of `story:sync-holds-back-secrets`, which this story waits for, holds back
credentials before any file reaches a machine the operator pays for), § 4.5 (clocks, seeding across commits), § 5.6 (sccache
across workspace ids), § 7.3 (boot steps), § 8.1 (the link check), § 10 questions 1 to 4.

## Acceptance

1. A provider-neutral runbook page, `website/docs/cluster/hand-started-node.md`, lists the
   pre-flight, the boot steps of § 7.3, the ssh options of § 9.1 step 4 with placeholders for
   every account value, and the measurements of step 7; `task public-safety` passes on it.
2. A `verification-report` in this store, from the run, records: the first sync, a no-change
   sync and a one-file sync of each worktree; median and p90 overhead over 20 warm calls (client
   wall time minus the node's enqueue-to-end); a cold build on the node against the same build
   on the workstation; the clock difference; twelve jobs at once from different sessions; the
   result of the seeding-across-commits check of § 4.5 (the second worktree compiles every crate
   that differs, or it does not); sccache hits across workspace ids with and without
   `CARGO_TARGET_DIR` (§ 5.6).
3. Each `(unverified)` mark of § 9.1 step 7 that the run answered is replaced in the design by
   the measured value or the observed behaviour, citing the report.
4. After the deadline the provider lists no machine and no resource tagged for the pool; the
   report says so.

## Scope

`website/docs/cluster/hand-started-node.md`, `website/sidebars.ts`, `docs/design/remote-pool.md`.

`website/sidebars.ts` is also edited by `story:prometheus-metrics` (another epic): one line each,
whichever merges second rebases. This story overlaps `story:remote-mode-on-a-real-server` (a real
second machine, overhead against G4); its report says which of that story's acceptance items the
same measurement answered.

## Blocked on

`decision-blocker:phase-0-node-launch`: the machine is the operator's spending decision.

## Not in this story

Daemon code. The account values stay in the operator's own configuration.
