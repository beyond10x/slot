---
format: aep.planning-md/3
id: story:cancelled-job-leaves-no-process
kind: story
status: draft
title: A cancelled job leaves no process behind
relations:
- serves: vision:O2
revision: 1
---
## Outcome

A cancelled job leaves no process behind: every process in the job's process group is gone at
most 5 s after SIGTERM, also when the job's main process exits on SIGTERM and a process in its
group does not.

## Context

- `AGENTS.md`, Boundary: "After a normal exit the daemon does not kill the job's process group: a
  sccache server started by that job lives in it. On cancel the group is killed."
- Seen on one host: two `cargo test -p b10x-slot --lib` jobs whose clients went away were recorded
  `cancelled` (job log `jobs.jsonl`): job 6612 (started 11:08:24, ended 11:25:48) and job 6765
  (started 11:24:03, ended 11:25:47). Their test binaries, hung on a deadlock in the code under
  test, still ran 1 h 47 min and 1 h 31 min after they started, in the daemon's cgroup, with the
  job's process group id, whose leader (`cargo`) was gone.
- `server/mod.rs` `supervise`: on client loss the group gets `SIGTERM`; `SIGKILL` follows only if
  the main process has not exited within 5 s. `cargo` exits on SIGTERM, so SIGKILL is never sent.
- Inferred, not verified: the test binary does not die on SIGTERM (a test in the library installs
  the client's signal handling, which records SIGTERM instead of exiting).

## Acceptance

1. A test job whose main process exits on SIGTERM and whose child ignores SIGTERM (`sh -c 'trap ""
   TERM; sleep 600 & wait'` or a fixture binary) is cancelled; 6 s later no process of its group
   exists (`kill(-pgid, 0)` fails with ESRCH).
2. A job that exits normally keeps its group, as today (an sccache server it started survives).
3. `slot status` and the job log name a cancelled job whose group needed SIGKILL.

## Scope

- `crates/slot/src/server/mod.rs` (`supervise`), `crates/slot/src/server/tests.rs`
