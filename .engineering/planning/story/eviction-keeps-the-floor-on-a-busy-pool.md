---
format: aep.planning-md/3
id: story:eviction-keeps-the-floor-on-a-busy-pool
kind: story
status: draft
title: Eviction keeps the disk over its floor while the pool is busy
summary: delete targets of busy repositories that no queued or running job builds into
relations:
- serves: vision:O2
revision: 1
---
## Outcome

Eviction keeps the disk over its floor while the pool is busy. It may delete a target directory of
a repository that has jobs queued or running, as long as no queued or running job builds into
that directory, and it prefers the directories with the most bytes per hour since their last
build.

## Why

On 2026-11-23 the root filesystem fell from 51 GB free at 13:04 CET to 34 GB at 13:31, under the
disk hold of 40 GB, while `slot status` listed 5,982 known target directories. Eviction had
deleted 1 directory and 0.0 GB since the daemon started at 12:11. It skips every directory whose
repository has a job queued or running, and every one built in the last two hours
(`crates/slot/src/evict.rs`, module comment). With 43 jobs queued and 20 running across about 20
repositories, every large target was skipped, so the pool held new builds for disk and nothing
freed it.

## Acceptance

- A test: two target directories of one repository, a job queued that builds into the first;
  under the floor, eviction deletes the second and keeps the first.
- The two-hour rule stays for directories a queued or running job builds into; other directories
  of the same repository are eligible after 30 minutes without a build.
- `slot status` names the reason when eviction cannot reach the floor: how many directories and
  GB each rule kept.
- Measured on this host for one busy hour: GB deleted and free space at the floor, before and
  after.
