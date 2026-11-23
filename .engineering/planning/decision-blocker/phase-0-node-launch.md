---
format: aep.planning-md/3
id: decision-blocker:phase-0-node-launch
kind: decision-blocker
status: cleared
title: The phase-0 node is a paid machine only the operator starts
relations:
- blocks: story:one-hand-started-node-measured
withholds: metric_observation
revision: 2
transitions:
- {from: "open", to: "cleared", at: "2026-11-23T22:54:29Z", actor: "human:timo", revision: 2}
---
# Decision blocker: the phase-0 node is a paid machine the operator starts

`story:one-hand-started-node-measured` measures one cloud machine of about 64 vCPU and 128 GiB
for 4 hours, on demand, started by hand (`docs/design/remote-pool.md` § 9.1; about $13 at an
approximate list price). Starting it spends money, and the account, the budget alert and the
launch values are the operator's (§ 10 question 6). They live in the operator's host
configuration, never in this repository.

## The question

When does the operator start the phase-0 machine, in which of his accounts, and with which budget
cap for the night?

## Clears when

The machine answers `ssh <node> true` from the workstation with its end armed (its shutdown
timer set to the deadline), and the operator has named the cap.
