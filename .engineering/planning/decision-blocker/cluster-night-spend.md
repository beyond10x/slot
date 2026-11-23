---
format: aep.planning-md/3
id: decision-blocker:cluster-night-spend
kind: decision-blocker
status: open
title: The acceptance night runs two paid machines only the operator starts
relations:
- blocks: story:cluster-night-measured
withholds: metric_observation
revision: 1
---
# Decision blocker: the acceptance night runs two paid machines the operator pays for

`story:cluster-night-measured` measures one night of at least 4 hours with two remote nodes of
the operator's templates (`docs/design/remote-pool.md` § 9.5). The nodes are paid machines; the
templates, the provider's account values, the daily cap and the budget alert are the operator's
and live in the host configuration, never in this repository (§ 10 question 6).

## The question

Which night, with which two templates, and under which daily cap?

## Clears when

The operator has named the night and the cap, and `slot cluster config check` passes on the host
configuration that holds them.
