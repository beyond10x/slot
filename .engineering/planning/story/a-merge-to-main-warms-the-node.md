---
format: aep.planning-md/3
id: story:a-merge-to-main-warms-the-node
kind: story
status: active
title: A merge to a repository's main warms the node for the next session
relations:
- serves: vision:O6
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-11-24T13:09:05Z", actor: "agent:slot", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-24T13:09:06Z", actor: "agent:slot", revision: 3}
---
# A merge to a repository's main warms the node for the next session

## Why

On 2026-11-24, 44 of 69 cargo working directories on the build node were first used within one
hour; 314 cargo jobs ran in them. The first build of a new worktree compiles from a cold or
seeded target directory while its session waits.

## Acceptance

- After a merge to `main` of a repository that built on the node in the last 24 hours, slot starts
  one low-priority job on the node that builds the tests of `main` (`cargo test --no-run` or the
  repository's warm-up task named in its slot config), into the target directory new worktrees of
  that repository are seeded from.
- The warm-up job never delays a job of a session: it runs at the lowest priority and is cancelled
  when a session's job waits for its slots.
- A repository can name its warm-up command or switch it off in its slot config.
- Red first: a test daemon told of a merge starts one warm-up job of the lowest priority, and a
  job a session submits next preempts it.
- Measured after: the share of cargo jobs whose target directory started empty.
