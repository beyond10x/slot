---
format: aep.planning-md/3
id: story:cluster-view-in-web-ui
kind: story
status: draft
title: An animated cluster start page, and a node filter in the job views
relations:
- decomposes: epic:compute-cluster
- serves: vision:O2
- informed_by: executable-system-specification:compute-cluster
- depends_on: story:daemon-knows-nodes
scope:
- confidence: inferred
  path: docs/design/assets
- confidence: inferred
  path: ui/src/App.vue
- confidence: inferred
  path: ui/src/components
- confidence: inferred
  path: ui/src/store.ts
- confidence: inferred
  path: ui/ux/baseline.json
- confidence: inferred
  path: ui/ux/scenarios.ts
- confidence: inferred
  path: ui/ux/tasks.ts
revision: 9
---
# Story: an animated cluster start page, and a node filter in the job views

## Outcome

The web UI opens on a new cluster start page, the design the operator approved on 2026-11-23
(`docs/design/assets/cluster-start-draft.html`, screenshots `cluster-start-draft-1366.png` and
`cluster-start-draft-filter.png` beside it). Every existing view stays reachable and unchanged.
The page shows the endpoint's queue, each node as a card, jobs moving from the queue to the node
their worktree is placed on, a row of headline numbers and a newest-first job list; choosing a
node filters the page and the existing job views to it.

## Specification

`spec/domains/cluster.yaml` views ClusterStatus, NodeList, NodeDetail, PlacementsOnNode, Jobs,
JobsOnNode; Node fields `kind`, `cpus`, `slots`, `slots_in_use`, `memory_gib`, `disk_gib`,
`cpu_busy_milli`, `memory_used_mib`, `disk_free_gib`, `location`, `machine_type`,
`price_per_hour_cents`, `joined_at`, `deadline`, `left_at`, `gone_reason`; Job fields `node_id`,
`worktree_id`. Every field exists in the daemon's API from `story:daemon-knows-nodes`; later
stories fill values (placements, gone reasons, provider labels, remote load), so this page is
built on the scenario data of acceptance 1 and shows them as they arrive, with no further change.

## Design

`docs/design/assets/cluster-start-draft.html` (the approved draft, simulated data);
`docs/design/remote-pool.md` § 3.7 (one line per node: state, placed worktrees, running and
queued jobs, deadline); `AGENTS.md` § UX gates and rounds. The draft's simulation holds two
scheduling rules marked NOT ADOPTED (a worktree moving after 12 s on a full node; a joining node
taking idle worktrees); the page draws placements as the daemon reports them and moves nothing
itself.

## Acceptance

1. Before the UI change, `ui/ux/scenarios.ts` holds a scenario with the local node and three
   remote nodes (one Ready with three placements, a location and a machine type; one Joining;
   one Draining 20 minutes before its deadline), one Gone node with reason Lost, queued and
   running jobs on each, and the cache counters of the last 24 h; the change that adds the page
   is a separate commit after it.
2. The start page shows:
   - an endpoint panel with the queue, newest job first, each job naming its worktree, slots and
     the node it waits for, and a 60-second line of the queue length;
   - one lane per placed worktree from the endpoint panel to its node's card, along which a
     job's mark moves when it starts;
   - per node a card with its kind, CPUs, slots, memory and disk, its state in words (joining,
     ready, draining, gone), one cell per slot marked busy or free, a 60-second line of
     `cpu_busy_milli`, bars of memory used and disk free, the worktrees placed on it, and for a
     remote node a ring with the time left to its deadline and the countdown in `h:mm` (a local
     node shows no deadline);
   - headline numbers that update with each state tick: running jobs, queued jobs, queue wait
     p90, CPUs in use of CPUs available, compile-cache hits of the last 24 h, and the cost of the
     cloud nodes so far (each node's `price_per_hour_cents` from `joined_at` to `left_at`, or to
     now);
   - a job list, newest first, with a node column.
3. Clicking a node card, or Enter on it, shows only that node's jobs in the job list and dims the
   other cards; Esc, or the chip's close button, clears it. The same node filter in the existing
   job views shows only jobs whose `node_id` is that node. Each is one task in the screen-only
   bot, done by pointer and by keyboard in at most 2 actions.
4. Dark is the default theme, with a toggle to light. Under `prefers-reduced-motion: reduce` no
   mark moves along a lane and no number counts: changes fade.
5. At 1366x768 and 1280x720 the start page has no scroll of the page in either direction and
   nothing past its card (the gates of `ui/ux/gates.spec.ts`), with the scenario of acceptance 1;
   axe-core finds 0 serious or critical issues in both themes.
6. Every existing view and its UX gates are unchanged and reachable from the start page in one
   action; `task ui-ux` passes and `node ux/ratchet.ts` passes, or the new baseline is committed
   in the same change.
7. The page names no repository or worktree but those of the scenario data (`repo-a/feature-x`
   style), and `task public-safety` passes.

## Scope

`ui/src/components/`, `ui/src/store.ts`, `ui/src/App.vue`, `ui/ux/scenarios.ts`,
`ui/ux/tasks.ts`, `ui/ux/baseline.json`, `docs/design/assets/`.

It reads `ui/src/generated/cluster` (the spec's TypeScript) and `ui/src/generated/types.ts` as
`story:daemon-knows-nodes` leaves them and does not
regenerate it; `story:placement-sticks-to-a-node` regenerates it and depends on this story.
`ui/src` is also in the scope of `story:admission-waits-for-memory` (another epic); whichever
merges second rebases.

## Not in this story

The API itself (`story:daemon-knows-nodes`); the remote node's load values
(`story:node-health-poll`); any rule that moves a worktree between nodes.
