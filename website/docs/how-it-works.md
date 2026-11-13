---
title: How it works
description: The jobserver pool, how jobs take slots, the queue, the job log and the web UI.
---

# How it works

```text
slot cargo test ──unix socket──► slot serve
                                   │ toolchain: rust, holds 1 slot
                                   │ queue until the slot is free
                                   │ start: MAKEFLAGS=-j --jobserver-auth=fifo:<state-dir>/jobserver.fifo
                                   │        cargo, rustc take more tokens from the FIFO
◄── stdout, stderr, exit code ─────┘ job log: one JSON line
```

## The pool

`slot serve` creates one named FIFO, `<state-dir>/jobserver.fifo`, and writes one token into it
per slot. The number of slots is `--tokens`, by default the number of CPUs the daemon may use.

This is the GNU-make jobserver protocol. Every job starts with

```text
MAKEFLAGS=-j --jobserver-auth=fifo:<state-dir>/jobserver.fifo
CARGO_MAKEFLAGS=-j --jobserver-auth=fifo:<state-dir>/jobserver.fifo
```

A tool that speaks the protocol (cargo, rustc, GNU make 4.4 and newer) reads a token from the FIFO
before it starts another unit of parallel work and writes it back when that work ends. All jobs
read from the same FIFO, so all of them together never run more parallel work than there are
slots. Twelve `cargo test` runs on a 20-slot pool share 20 slots; they do not get 20 each.

The daemon drops the caller's `MAKEFLAGS`, `MFLAGS`, `CARGO_MAKEFLAGS` and `CARGO_BUILD_JOBS`
before it starts a job. Any of them would let a job bypass the pool.

## Slots per job

Before a job starts, the daemon takes the slots the job's [toolchain](toolchains.md) asks for and
holds them for the whole run:

| toolchain | commands | slots held | parallelism beyond that |
|---|---|---|---|
| `rust` | `cargo`, `cargo-*`, `rustc` | 1 | cargo and rustc read more tokens from the FIFO |
| `go` | `go`, `gotestsum`, `golangci-lint` | `--go-tokens`, default 4 | none: `GOFLAGS` gets `-p=<slots held>` |
| `other` | anything else | 1 | jobserver clients started inside share the pool |

A job never holds more slots than the pool has: a go job on a pool of 2 holds 2 and runs with
`-p=2`.

For `rust` jobs the daemon also sets `RUST_TEST_THREADS` and `NEXTEST_TEST_THREADS` to the number
of free slots when the job starts, at least 2, unless the caller set them. The jobserver does not
limit the threads of a test binary; this does.

## The queue

A job whose slots are not free waits. Jobs are admitted in the order they arrived. While a job
waits, `slot` prints nothing; when it ends after a wait of two seconds or more, `slot` prints one
line:

```text
slot: job 3 waited 5s for a build slot, ran 0s
```

When the client goes away (Ctrl-C, a killed agent, a closed ssh connection), the daemon cancels
the job: it kills the job's process group and returns the job's slots. `slot cancel <id>` does the
same from another shell. Only the user who submitted the job may cancel it: the daemon compares
the user id of the two socket connections (`SO_PEERCRED`).

After a normal exit the daemon does not kill the process group. An sccache server that the job
started lives in that group and serves later builds.

When no job runs and none waits, every token must be back in the FIFO. The daemon checks this and
restores tokens that a killed process took with it, or removes surplus ones, and logs
`slot: jobserver reconciled by <n> tokens`.

A busy pool is never idle, so the daemon also checks every 15 seconds while jobs wait and the FIFO
is empty. If no process but the daemon has the FIFO open, every token must be in the FIFO, held
back for headroom, granted to a running job, or taken by the first queued request; what is missing
was taken by a compiler that was killed (a cancel, a `timeout`). The daemon writes the smaller of
two shortfalls 15 seconds apart back and logs
`slot: <n> jobserver token(s) were lost … wrote them back`; `/metrics` counts them in
`slot_tokens_restored_total`. While any process has the FIFO open (a compiler, or a cargo nested in
a job that inherited the jobserver) it cannot tell a lost token from one in use, so it waits.

## sccache watchdog

An sccache server limits its own compiles with a private jobserver: a pipe that starts with one
token per CPU. A token a compile never gives back is gone until the server restarts, and once
all are gone every `sccache rustc` on the machine waits at 0 % CPU behind the compiles still
running. Restarting the server fails the compiles in flight.

Every 5 seconds the daemon looks at each sccache server of your user: the tokens in its pipe
(read through `/proc/<pid>/fd`), its running compiles (its child processes) and its clients
waiting on the same port. When for 60 seconds no token was free, fewer compiles ran than the
pool holds, a client waited and the server itself used under 5 % of a core, the daemon writes
the missing tokens back, at most `pool − running`. The pool is the number of CPUs, or more if
the server was seen with more. A server whose jobserver pipe cannot be told apart from its other
pipes is left alone.

Each refill is a journal line and a `"event":"sccache_refill"` line in `census.jsonl`.
`slot status` shows the servers, how many are short of tokens now, and the refills since the
daemon started:

```text
sccache: 2 server(s), 0 short of tokens, refills since start: none
```

`slot serve --sccache-watch false` turns the watchdog off.

## Waiting for disk and memory

Before a job takes its slots, the daemon checks two things, and the job waits while either is
short:

- **Disk.** The filesystem of the job's directory, and of `CARGO_TARGET_DIR` when the job sets
  it, must have 25 GB free (`slot serve --min-free-disk`, in GB; 0 turns it off). A job that
  waits for disk waits until space is freed.
- **Memory.** Available memory (`MemAvailable`), less what jobs that started in the last 30
  seconds are still expected to take, must cover the job's expected peak plus 4 GB
  (`--min-free-memory`, in GB; 0 turns it off). The expected peak is the 90th percentile of the
  peak memory of the same command in the same repository over its last 20 runs (at least 3),
  else of the same toolchain over its last 200 runs (at least 5), else 1 GB. A job is never held
  for memory while no other job runs, so the queue cannot stall on it.

The client prints why it waits, once per reason:

```text
slot: job 12 waiting: 18.0 GB free on the filesystem of /srv/build/repo, builds need 25.0 GB
```

`slot status` shows the reason under the queued job, and the web UI's queue marks it `disk` or
`memory`. The check runs again every 2 seconds, and once more after the slots are granted: a job
that finds memory gone by then gives its slots back and waits again.

## Freeing disk space

The daemon also frees disk. Every minute it looks at the filesystems of the Cargo target
directories jobs built into. When one has less than 40 GB free (`slot serve --evict-below`, in
GB; 0 turns it off), it deletes target directories there, least recently built first, until the
filesystem would have 50 GB free. It never deletes:

- a target directory of a repository with a queued or running job;
- one built in the last 2 hours;
- a directory no job built into, or one without the `CACHEDIR.TAG` Cargo writes.

A target directory inside a managed worktree is deleted with `worktree discard-cache`.

Eviction starts as a dry run (`--evict-dry-run true`): the journal and `slot status` list what
it would delete, and nothing is deleted. To let it delete, add `--evict-dry-run false` to the
`slot serve` line of the service and reload.

```text
disk: 35.2 GB free, under 40.0 GB: dry run, would delete 3 target dir(s), 21.4 GB
```

## Memory limit

`slot serve --memory-max 24G` runs each job in a systemd scope with that `MemoryMax`, so an
out-of-memory kill ends that job and not the daemon. The scopes run in `slot.slice`, outside the
daemon's unit, so the headroom's CPU quota is set on `slot.slice` as well as on the daemon's
unit. It is off by default and has not been tested beyond starting jobs.

## Job log

Every finished job is one JSON line in `<state-dir>/jobs.jsonl`. This one is from a job that
waited for a slot and exited with 3; the directory and client are shortened:

```json
{"id":3,"argv":["sh","-c","echo second job; exit 3"],"cwd":"<repo>","repo":"repo","session":"","client":"<host>:<pid>","toolchain":"other","remote":false,"tokens":1,"state":"failed","enqueued_ms":1794473158546,"started_ms":1794473163741,"ended_ms":1794473163763,"exit":3,"cpu_s":0.0,"compile_server_cpu_s":0.0,"peak_rss":0}
```

`enqueued_ms`, `started_ms` and `ended_ms` give the queue wait and the run time. `cpu_s` and
`peak_rss` are measured from `/proc` for the job's processes. `session` is the agent session that
started the job, when the client runs inside a Claude Code session (`CLAUDE_CODE_SESSION_ID`).

`compile_server_cpu_s` is CPU time spent for this job inside an sccache server, which runs outside
the job's process tree. Every job's environment carries `SLOT_JOB=<id>`; sccache passes the
caller's environment to its compiler processes, so the daemon can attribute them to the job.

The daemon reads the log back at start, so the web UI shows recent jobs across restarts.

## Web UI

The daemon serves a web UI on `127.0.0.1:7411` (`--http <address>`, `--http off` to disable). It
shows:

- slots in use out of the pool, and the **headroom for you** slider with buttons for 0, 20 and 40 %
  ([Headroom](headroom.md)); on the machine base, the line with the CPUs busy on the host, the
  target, the pool's quota and the busiest owners outside the pool,
- a timeline with one bar per job, coloured by toolchain; the pale part of a bar is the wait for
  a slot,
- cores in use by slot jobs per toolchain, and by the whole host,
- running and queued jobs: repository, command, state, wait, elapsed time, cores, memory, session,
- finished jobs: exit code, wait, duration, CPU seconds, average cores, peak memory,
- the live output of a job, opened by clicking its bar or its job number in a table; the job number
  is a button, so the keyboard reaches it with Tab.

It keeps the last 500 finished jobs in memory (`--keep-jobs`). The UI has no authentication;
keep it on a loopback address. The same data is available as JSON from `/api/state`, as a stream
of server-sent events from `/api/events`, and per job from `/api/jobs/<id>/output`. `/api/limits`
reads and sets the headroom.
