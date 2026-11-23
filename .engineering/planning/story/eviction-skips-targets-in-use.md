---
format: aep.planning-md/3
id: story:eviction-skips-targets-in-use
kind: story
status: draft
title: Eviction never deletes a target directory a process or a service uses
summary: skip targets that a process executes from, works in, holds open, or a unit's ExecStart names
relations:
- serves: vision:O2
revision: 1
---
## Outcome

Eviction never deletes a target directory that a running process uses or that a service will
start from: a process whose executable, working directory, open file or mapped file is inside it,
or a systemd user unit whose `ExecStart` names a path inside it.

## Why

On 2026-11-23 at 13:30:02 CET the daemon logged `disk: 40.0 GB free, under 40.0 GB: deleted
<home>/.local/share/<tool>/runbook/target (0.0 GB, last built 2h22m ago)`. A systemd user unit
ran a daemon from that directory and a command was executing from it; the next run failed with
exit 127. Eviction checks only the job log (no job queued or running for the repository, nothing
built in the last two hours); it does not look at processes or units (`crates/slot/src/evict.rs`).
The target directory beside it was next by the same rule. Eviction was set to a dry run in
`evict.json` on the live daemon until this is built.

## Acceptance

- A test: a candidate target directory with a process started from a binary inside it is kept,
  and the journal line names the process id; after the process ends, it is deleted.
- The same for a process whose working directory is inside it, and one that holds a file in it
  open.
- A unit file in the user unit directory whose `ExecStart` names a binary inside a candidate keeps
  that candidate, whether or not the unit runs.
- `slot status` counts the directories kept this way.
- `evict.json` goes back to deleting once released and installed.
