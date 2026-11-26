---
format: aep.planning-md/3
id: story:a-pending-node-restart-happens-without-a-second-apply
kind: story
status: draft
title: A pending restart of the node's daemon happens without a second apply
relations:
- decomposes: epic:compute-cluster
- serves: vision:O2
revision: 2
---
## Outcome

A change that needs a restart of the node's daemon (a new group of the build user, a new limit
of the unit, a replaced binary) takes effect after one `apply --drain` on a node that always has
a job: the daemon stops taking jobs, the jobs it has end by themselves, the daemon starts again,
and a job sent meanwhile waits on the workstation and runs on the node afterwards.

## Why

`provision.sh apply` restarts `slot-node.service` only when the pool counts 0 jobs at that
moment. Otherwise it prints `pending: running: slot-node restarts when no job runs (<n> now)`
and exits 0. Observed: after an `apply` that added the build user to a group, 10 further runs of
`apply` within 29 minutes each found 5 to 14 jobs and printed the pending line; the pool never
counted 0, and `check` kept reporting `slot-node.service: the process started before <user> was
in group docker`.

A drain alone is not enough: a job sent to a node whose daemon has no socket ends at once with
exit 3 (`slot: connection to daemon lost`).

## Acceptance

- `apply --drain` with a pending restart and running jobs signals the daemon alone, prints
  `draining: slot-node: <n> job(s), restart by <UTC time>` and ends; no job is signalled.
- The daemon starts again by itself after its last job ended, with the unit file and the groups
  of that moment; a pending restart of the compile cache's unit happens while no daemon runs.
- A drain that the same daemon process has not ended after `DRAIN_MAX` (config, default 30m) is
  ended by a restart of the unit; a daemon that already restarted is left alone.
- `check` reports a running drain as drift with the jobs left and the bound; after the restart
  it reports none from the drain.
- `apply` without `--drain` behaves as before.
- A job sent to a node whose daemon does not answer is sent again until it answers, within a
  bound, and does not run on the workstation; an `ssh` that fails itself ends the call as before.
- Red first: tests in `contrib/node/test.sh` for the drain and tests of the client for the wait
  fail before the change.

## Notes

- A reload with handover keeps the process and its groups and limits; only a full restart of
  the unit applies them.
- The daemon drains on SIGTERM: it removes its socket, waits for running and queued jobs, and
  exits. The unit has `KillMode=control-group`, so `systemctl restart` signals every job; the
  drain signals the main process only.
