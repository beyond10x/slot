---
format: aep.planning-md/3
id: story:a-lease-end-stops-a-node-and-never-deletes-it
kind: story
status: draft
title: A lease end stops a node and never deletes it, and one place holds the end
relations:
- serves: vision:O2
revision: 1
---
## Outcome

A node's lease end stops the instance and never deletes it, and one place holds the end: moving
it on the workstation moves it on the node.

## Why

On 2026-11-26 a node had a poweroff scheduled on the node itself for its lease end (logind, set
by hand as the README's step 9 says), the workstation held the same time in two files, and the
instance's shutdown behaviour was `terminate` with no termination protection. The owner decided
46 minutes before the end to keep the node. The two workstation values were about to be moved
while the poweroff on the node stayed; a read 45 minutes before the end found it. The poweroff
would have deleted the instance, its root volume and its instance store.

## Acceptance

- The README's launch step names the instance attributes a node needs: shutdown behaviour
  `stop` and termination protection. `role.sh check` reports an instance without them as drift
  and `role.sh plan` prints the two commands.
- The lease end is a key of the node's config: `provision.sh apply` schedules the poweroff for
  it, and `check` reports a scheduled poweroff that differs from the key, or one that exists
  without the key, as drift with both times.
- `push.sh check` reports a lease end on the node that differs from the workstation's
  `cluster.toml` entry of that node.
- Tests in `contrib/node/test.sh` for each line; no test calls AWS.
