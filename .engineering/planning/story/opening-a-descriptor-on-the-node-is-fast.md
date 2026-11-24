---
format: aep.planning-md/3
id: story:opening-a-descriptor-on-the-node-is-fast
kind: story
status: active
title: Opening a file descriptor on the node is as fast as on the workstation
relations:
- serves: vision:O6
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-11-24T15:37:55Z", actor: "agent:slot", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-24T15:37:55Z", actor: "agent:slot", revision: 3}
---
# Opening a file descriptor on the node is as fast as on the workstation

## Why

On 2026-11-24 a session measured on the build node that the first open of file descriptor 64, 128 or 256 in a process waits 108-142 ms; latency tests see it. That is the time the kernel takes to grow the descriptor table under RCU (a grace period). The cause on the node (kernel version, `rcu_*` boot options, CPU count) is not established.

## Acceptance

- A probe that opens 300 descriptors in a fresh process reports the slowest open on the node and on the workstation; the node is within 5 ms of the workstation.
- The cause and the change (a kernel parameter such as `rcupdate.rcu_expedited=1`, or a node kernel) are written in `docs/design.md`.
- The probe runs in the node health check, outside the test step.
