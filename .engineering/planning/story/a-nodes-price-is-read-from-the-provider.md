---
format: aep.planning-md/3
id: story:a-nodes-price-is-read-from-the-provider
kind: story
status: draft
title: A node's price is read from the provider, not typed into cluster.toml
relations:
- serves: vision:O2
revision: 1
---
# A node's price is read from the provider, not typed into cluster.toml

## Why

`price_per_hour_cents` in `cluster.toml` is written by hand. On 2026-11-24 a build node went from
a 64-CPU to a 32-CPU instance type of the same family; the price was changed from 373 to 187
cents by halving it, not read from any price list. The live view and the spill decision show
that number as the node's cost.

## Acceptance

- The node reports its machine type and region in its `slot shell` state answer, read on the
  node; `machine_type` in `cluster.toml` is no longer needed for it.
- slot looks up the hourly on-demand price for that type and region in the provider's public
  price list (no credentials) and keeps it with the list's date.
- The live view and `slot status` show the price with its source: `price list <date>` or
  `cluster.toml`.
- A `price_per_hour_cents` in `cluster.toml` that differs from the price list is reported once
  per daemon start, naming both numbers.
- Red first: a test node whose reported type changes from a 64-CPU to a 32-CPU type shows the
  list price of the new type without an edit of `cluster.toml`.
