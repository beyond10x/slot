---
format: aep.planning-md/3
id: story:a-pool-job-stays-on-this-host-for-a-named-reason
kind: story
status: draft
title: A pool job stays on this host for a named reason
relations:
- serves: vision:O2
revision: 1
---
# Story: a-pool-job-stays-on-this-host-for-a-named-reason

## Outcome

`slot --here` keeps a job on this host because it needs this host's credentials, and a compile under
`--here` is refused (story a-compile-under-here-is-refused). That leaves a session no form that runs a
compile on this host inside the pool when a build node lacks a tool, a library or a cache the
compile needs: the only forms left run outside the pool (`slot --local`, the shim switch) or need
the operator to add the tree to the remote deny list.

A second option, `slot --stay <reason>`, keeps the job in this host's pool and off every node. The
reason is required, one line, and is the job's `local_reason` (`stay: <reason>`), so `slot status`,
the job log and `slot usage` show why the job did not run on the node and how often each reason
occurs. A reason that names a node gap is a defect report that the operator can count.

## Evidence

- A compile that fails on the node for a missing tool is rerun by its session with `slot --local`,
  outside the pool, with no slot held and no queue position (2026-11-25: two sessions stopped on
  the `--here` refusal, as the operator reported).
- The remote deny list is a file the operator edits; a session cannot use it for one rerun.

## Acceptance

- `slot --stay "<reason>" cargo build` runs in this host's pool, takes a slot, is never handed to a
  node, and `slot status` shows `local: stay: <reason>` under it.
- `slot --stay` without a reason, or with an empty or multi-line one, exits 2 and prints the usage
  line; it does not run the job.
- The job record carries the reason; `slot usage` groups jobs that stayed by reason.
- `slot --stay "<reason>" git fetch` and `slot --here …` behave as before; `--stay` and `--here`
  together are refused.
- The shim passes the option through, so `slot --stay "<reason>" cargo test` is the one-line rerun
  after a node gap.

## Out of scope

- Closing the gap on the node: that is a change to the node's provisioning.
- A time limit or a count limit on `--stay` jobs: the reason counts in `slot usage` first.
