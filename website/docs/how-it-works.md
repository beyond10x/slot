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

One setting changes this: with [`slot job-parallel`](#build-processes-per-job) on, a job gets a
bound of its own instead of the pool's FIFO.

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

## Build processes per job

A `rust` or `other` job holds one slot, and its `cargo` reads one more token from the FIFO for
every compiler it starts. A freed token goes to whoever reads the FIFO first, and the `cargo`
processes of the running jobs read it against the one daemon that reads for the waiting jobs. On
a busy pool the running jobs grow wider while the queue stands: on one pool of 50 slots, 7 `cargo`
processes read against the daemon, one of them ran 19 compilers, and 10 jobs ran while 49 waited.

`slot job-parallel` takes the compilers out of the pool. It is off on a fresh install, and off is
the behaviour described above, unchanged.

```sh
slot job-parallel        # print the value in force
slot job-parallel 4      # a job runs at most 4 build processes, on a bound of its own
slot job-parallel off    # back: every build process takes a slot of the pool
```

With the setting at N the daemon starts a job without the pool's jobserver. In place of
`MAKEFLAGS` and `CARGO_MAKEFLAGS` with `--jobserver-auth`, every job gets

```text
CARGO_BUILD_JOBS=<P>
MAKEFLAGS=-j<P>
```

P is N, or the slots the job holds when those are more: `slot --tokens 8 cargo build` gets 8, a
go job that holds 6 slots gets 6. cargo finds no jobserver in its environment, makes one of its
own with P tokens and hands that to rustc and to build scripts; GNU make runs P recipes at once.
The job still waits in the queue and holds its slots as before. Only its compilers stop reading
the pool's FIFO, so the daemon is the FIFO's only reader and a freed slot goes to the head of
the queue.

The bound needs the pool's jobserver gone: cargo ignores `CARGO_BUILD_JOBS` and `-j`
while it inherits a jobserver, so a bound per job cannot be had inside the pool.

What it costs:

- **The pool bounds jobs, not compilers.** A pool of S slots runs up to S jobs of one slot, and
  each runs up to N build processes: up to S × N at once. 50 slots at N = 4 allow 200 compilers
  where the pool alone allowed 50. Size `--tokens` or the headroom for that; the `CPUQuota` of
  the [headroom](headroom.md) still caps the CPU time of all jobs together.
- **A job no longer yields to the others.** With the pool's jobserver a `cargo` gets fewer tokens
  when the pool is full. With its own it runs P wide whatever else runs.
- **The bound is per `cargo` and per `make`, not per job.** A script that runs two `cargo`
  commands at once runs 2 × P compilers. A build script that uses the `cc` or `cmake` crate stays
  inside its cargo's jobserver; one that starts `make` itself and passes no jobserver on gets
  `-j<P>` from the job's environment, beside its cargo's P.
- **rustc inside the bound.** rustc takes tokens of its cargo's jobserver for its code generation
  threads, as it took the pool's before.

What it gives besides the queue order: a `cargo` that is killed takes no slots of the pool with
it, because it never held any. Lost slots (the end of [Priority](#priority) describes them) then
come only from jobs that started before the switch.

The setting applies to the jobs that start after the change; a running job keeps the environment
it started with. It is kept in `limits.json` in the state directory, beside the headroom, and
holds across reloads and restarts. `GET` and `PUT /api/job-parallel` read and change it
(`{"job_parallel": 4}`; `null` switches it off; a value under 1 or a body without the field is
refused with 400). `slot status` shows it in a header line when it is on:

```text
at most 4 build processes per job, outside the pool's slots
```

sccache from 0.12.0 leaves `CARGO_BUILD_JOBS` out of its cache key, so the setting changes
nothing the compile cache answers. An older sccache hashes it: there each bound has cache entries
of its own.

## The queue

A job whose slots are not free waits. While a job waits, `slot` prints nothing; when it ends
after a wait of two seconds or more, `slot` prints one line:

```text
slot: job 3 waited 5s for a build slot, ran 0s
```

### Priority

Every job has a level: `urgent`, `high`, `normal` (the default) or `low`. Queued `urgent` jobs get
free slots first, then `high`, `normal` and `low` ones; inside a level, jobs are admitted in the
order they arrived. A `normal` or `low` job that has waited more than 30 minutes (a delay that
`slot priority promote-after` changes, see below) counts as `high`, ahead of every `high` job that
arrived after it, so no level waits forever. A running job is never stopped for a queued one.

`urgent` is for repair work on the pool itself. When no slot is free, an `urgent` job starts at
once on a slot the [headroom](headroom.md) holds back, instead of waiting for a running job to
end. At most the held-back slots (the pool less the slots in circulation) are in use that way at a
time; each goes back to the held-back set when its job ends. Without headroom nothing is held
back, and an `urgent` job only goes to the head of the queue.

A job asks for its level with `slot --priority <level> <command…>`, `slot run --priority <level>`
or `SLOT_PRIORITY=<level>`; the flag wins over the variable. A rule for the job's repository wins
over both:

```sh
slot priority set repo-a high    # every job of repo-a
slot priority set 'web-*' low    # a glob: * is any run of characters
slot priority unset 'web-*'
slot priority list
```

A pattern matches the repository name `slot status` shows. An exact name wins over a glob, a
longer glob over a shorter one. The rule table starts empty, changes without a restart, re-ranks
the queued jobs at once, and is kept in `priority.json` in the state directory. `GET` and `PUT`
`/api/priority` read and change it (`{"pattern": "repo-a", "level": "high"}`; a `null` level
removes the rule). `slot status` ends each queued line with the job's level, and `high (waited)`
for a job raised because it waited; the web UI's queue has a `level` column. Both list the queue
in the order it is served.

The delay before a waiting job counts as `high` starts as the daemon's option `slot serve
--promote-after` (seconds, 1800 by default; 0 never raises a job). Change it without a restart:

```sh
slot priority promote-after 300      # raise a job after 5 minutes of waiting
slot priority promote-after 0        # never raise a job for waiting
slot priority promote-after default  # back to the daemon's --promote-after
```

Each prints the delay now in force, such as `a job queued longer than 5m00s goes as high`. The
new delay applies to the jobs already queued, within a second. It is kept in `priority.json`
beside the rules and wins over `--promote-after` after a reload or a restart, until `default`
removes it from the file. `slot priority list` and `GET /api/priority` (`promote_after_s`) show
the delay in force. `PUT /api/priority` sets it with `{"promote_after_s": 300}`, and a `null`
value returns to the daemon's option; the body may name a rule as well, and a body with neither
a `pattern` nor `promote_after_s` is refused (400).

A job that needs more slots than are free collects them as they come free. For its first 5
minutes in the queue (`slot serve --pass-for`, seconds, 300 by default) it does not keep them from
jobs of its own level that queue after it: a later job of its level that fits in the slots it
collected takes them and starts. With 4 slots, one running job on 1 and a job queued for all 4,
three 1-slot jobs of its level that queue behind it start at once. A job of a lower level never
starts while it waits. Once it has waited 5 minutes, no job that came after it starts before it: it
keeps every slot that comes free until it has all it asked for. A later job of a higher level still
goes first; after 30 minutes the waiting job is `high` itself.

Inside a level, a job that is predicted to run 5 seconds or less starts ahead of the others. The
prediction is the 90th percentile of the last 20 runs of the same command in the same repository,
with at least 3; a command with fewer runs has none. A job passed over this way also starts before
every later job once it has waited 5 minutes, so a stream of short jobs cannot hold it longer.

With `--pass-for 0` nobody passes: jobs start by level, then in the order they arrived, and a job
that needs 4 slots is not overtaken by later 1-slot jobs of its level or below.

`slot status` ends a queued line with `, holds 11 of 17` for the slots the job collected and
`, moved ahead` for a job that passed an earlier one, and its first line counts the collected
slots (`, 11 collected by queued jobs`). The web UI's queue shows `11/17` in its slots column
and `ahead` beside the level. Both list the queue in the order it is served, a job moved ahead
before the jobs it passes. The job log records the prediction (`predicted_ms`) and whether the
job was moved ahead (`ahead`).

When the client goes away (Ctrl-C, a killed agent, a closed ssh connection), the daemon cancels
the job: it kills the job's process group and returns the job's slots. `slot cancel <id>` does the
same from another shell. Only the user who submitted the job may cancel it: the daemon compares
the user id of the two socket connections (`SO_PEERCRED`).

After a normal exit the daemon does not kill the process group. An sccache server that the job
started lives in that group and serves later builds.

When no job runs and none waits, every token must be back in the FIFO. The daemon checks this and
restores tokens that a killed process took with it, or removes surplus ones, and logs
`slot: jobserver reconciled by <n> tokens`.

A busy pool is never idle, so the daemon also counts every 15 seconds while jobs run. A token is
in the FIFO, held back for headroom, granted to a running job, taken by the first queued request,
or in a process that has the FIFO open. What is in none of these was taken by a process that was
killed (a cancel, a `timeout`).

The daemon cannot read how many tokens a live process holds, so it counts the most each can hold
and never writes back a token a live process may have:

| process with the FIFO open | counts for |
|---|---|
| `cargo`, `make` or `ninja` | its child processes, less one |
| any other process, such as `rustc` | the larger of its child processes and its threads, less one |
| either, with more than one thread | one more |

The first unit of work runs on the token the process was started with, hence "less one". The
child processes count whatever they are: a `cargo` whose children are sccache clients holds a
token for each after the first, with no compiler below it. A process that holds its tokens for
minutes without using a CPU keeps its share; only its end removes it.

What is missing beyond that sum is lost. The daemon writes the smaller of two such shortfalls
15 seconds apart back, so a lost token is in circulation again within 45 seconds, and logs
`slot: <n> jobserver token(s) were lost … wrote them back`; `/metrics` counts them in
`slot_tokens_restored_total`. Between the two counts `slot status` shows
`lost slots: <n> that no job and no live process holds, oldest for <time>`, and `/metrics` the
gauge `slot_tokens_unaccounted`. Because the count per process is an upper bound, up to one lost
token for each process with several threads stays out until such a process ends.

The same count finds a surplus: more tokens than the pool has, after a slot was released twice.
The daemon takes out the smaller of two surpluses 15 seconds apart, from the FIFO or as the next
tokens come back, and logs
`slot: <n> jobserver token(s) more than the pool's <size> … took them out`.

## Run-time limit

A job that hangs holds its slots until somebody cancels it. The run-time limit ends it instead:
the daemon stops a job that has run longer than the limit. The time counts from the job's start;
the wait in the queue is not part of it. A fresh install has no limit. Set one while the daemon
runs:

```sh
slot run-limit              # print the limit in force
slot run-limit 10m          # end a job that runs longer than 10 minutes
slot run-limit 10m --max 2h # and let a job name up to 2 hours for itself
slot run-limit off          # no limit
```

A duration is a number of seconds, or a number with `s`, `m` or `h`: `90s`, `10m`, `1h`. The limit
and the maximum are kept in `limits.json` in the state directory, beside the headroom, and hold
across reloads and restarts. `GET` and `PUT /api/run-limit` read and change them
(`{"run_limit_s": 600, "run_limit_max_s": 3600}`; a field left out keeps its value, and
`"run_limit_s": 0` turns the limit off). A limit longer than the maximum is refused (400). `slot
status` shows the limit in its header lines when one is set.

A job that needs longer names its own time, up to the maximum (1 hour until `--max` changes it):

```sh
slot --timeout 30m cargo test --workspace
SLOT_TIMEOUT=30m slot cargo test --workspace
slot run --timeout 30m cargo test --workspace
```

The flag wins over the variable. The time a job gets is its `--timeout` or `SLOT_TIMEOUT`, capped
at the maximum, else the limit. The client is told when the daemon caps it: `slot: --timeout 2h
capped at 1h, the most a job may name`. A job's own time applies with no limit set too, and may be
shorter than the limit. With no limit set and no `--timeout`, a job runs as long as it takes.

A `timeout` command in front of the job's command extends nothing: `slot timeout 1800 cargo test`
ends at the limit like any other job. When the command starts with `timeout` (as its first or
second word, as in `slot nice timeout 30m make`) and names more than the job's limit, the client
prints one line when the job is submitted:

```text
slot: this job ends after 10m; a wrapped `timeout` does not extend it, name the time with `slot --timeout 30m` (at most 1h)
```

The limit a job starts with stays its limit: `slot run-limit` changes the jobs that start
afterwards, never one that runs. A reload hands a running job to the new binary with its limit and
its start time.

When a job passes its limit, the daemon ends it the way `slot cancel` does: SIGTERM to the job's
process group, SIGKILL 5 seconds later, and the slots go back to the pool. The client exits with
124, the code `timeout` uses, after one line:

```text
slot: job 12 ended after 10m01s: over its run-time limit of 10m; name a longer one with `slot --timeout <DURATION>`, at most 1h
```

The job's record in the [job log](#job-log) has `"timed_out":true` and its limit as `limit_s`,
and the web UI lists it as failed with exit code 124.

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

Once a minute the daemon also appends one line per running sccache server to
`<state-dir>/cache.jsonl`: `t` (Unix milliseconds), `port`, `rust_hits`, `rust_misses`, `c_hits`,
`c_misses`, `not_cached` (the calls the server refused, by its reason), `cache_size` and
`max_cache_size` in bytes, and `restarted`, which is true when a counter is lower than in the
line before for the same port. sccache counts from its last start and keeps the counters in
memory only. The daemon reads them with `sccache --show-stats --stats-format json`, and only
from a port whose server process runs and accepts connections: with no server there, sccache
0.18.0 answers with zeros, as if a server had just started. A server whose process used no CPU
time since its last line gets no new line, because its counters cannot have moved and every
request restarts its idle timeout (`SCCACHE_IDLE_TIMEOUT`); a reading each minute would keep it
running for good. A server that does not answer within 5 seconds is skipped and named once in
the journal. The file is not rotated; a line is about 300 bytes.

`slot cache refused` reads that file and the job log and lists, per repository (`--by project`
per project), the calls the servers refused to cache by reason, with hits and misses, over a
window (`--since 24h`, `--json` for the whole answer). The counters belong to a server, not to a
job, so each minute's calls go to the jobs that ran in it and whose compile server worked for them
(`compile_server_cpu_s`). When all of them are of one repository, the minute counts for it in the
`exact` column; otherwise the minute is split across the repositories by each job's
compile-server CPU per second of its run times its seconds in the minute, in the `shared` column.
A `no job` line counts the minutes with calls and no such job. Under the table each reason has
one line with what a repository can do about it: for `incremental`, `CARGO_INCREMENTAL=0` in jobs
that do not edit and rebuild, as a CI runner does; `crate-type` (bin, proc-macro, dylib and
cdylib crates) has no remedy in the repository; `multiple input files` and `missing input` say
what sccache saw (more than one input file in one call; an input file that is not where sccache
looks, as with generated sources); any other reason is named as sccache prints it, with no remedy
known.

`slot serve --sccache-watch false` turns the watchdog and the counter log off.

## Waiting for disk and memory

Before a job takes its slots, the daemon checks two things, and the job waits while either is
short:

- **Disk.** The filesystem of the job's directory, and of `CARGO_TARGET_DIR` when the job sets
  it, must have 25 GB free (`slot serve --min-free-disk`, in GB; 0 turns it off). A job that
  waits for disk waits until space is freed. A job whose last 5 runs of the same command in the
  same repository used no compile server and built into no target directory waits at a lower
  floor, `--min-free-disk-light` (in GB, at most `--min-free-disk`; by default the same value).
  A first run waits at `--min-free-disk`. The wait reason names the floor: `builds need 25.0 GB`
  or `this job needs 10.0 GB (no compile in its last 5 runs)`.
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

A cargo build (`cargo build`, `check`, `clippy`, `test` and the like) also waits, without a
slot, while another job of the pool compiles in the same target directory: cargo would only
print `Blocking waiting for file lock on build directory` there. Its reason reads
`target directory in use by job 12`; it goes on when that job has compiled (cargo runs tests
without the lock) or ended, and the jobs behind it in the queue start meanwhile.

## Waiting for the host's CPUs

Slots bound what the pool starts, not what the host runs: a compile cache server compiles in a
process group of its own, and editors, browsers and builds outside the pool use CPUs too. `slot
cpu-limit` adds a third check after disk and memory. It is off on a fresh install, and off is
exactly the behaviour without it.

```sh
slot cpu-limit              # print the values in force and the host's busy CPUs
slot cpu-limit 17           # hold starts while the host is 17 or more CPUs busy
slot cpu-limit 17 --floor 2 # and let 2 jobs run whatever the host does
slot cpu-limit off          # no limit
```

With a mark set, a queued job that could start is held while both are true:

- **The floor runs.** At least `--floor` jobs are running. The floor is 4 until one is named, so
  a host that is busy with other work still builds.
- **The pool's work is at the mark.** The CPUs the pool's jobs and the compile cache server use,
  plus what the jobs that just started are expected to add, are at or above the mark.

What is counted:

- **The pool and the compile cache.** The figure is the CPU time of two parts: the processes of
  the pool's running jobs, with the compilers a compile cache server (sccache) runs for them, and
  the rest of the compile cache server's processes. Work outside both (editors, browsers, builds
  that do not go through slot) is not counted: the pool cannot make room for it by holding its own
  jobs. The daemon samples the parts once a second for the web view and the census log; the limit
  reads the mean of the last 10 samples and scans nothing itself. A process that starts and ends
  between two samples is not in it. `slot cpu-limit` prints both parts and the CPUs outside them:

  ```text
  starts held while the pool and the compile cache use more than 17.0 CPUs and 6 jobs run; now pool 12.0 + compile cache 6.0 of 20 CPUs, 1.5 outside both, 1 job held
  ```
- **Jobs that just started.** A job that started in the last 30 seconds counts with the CPUs it
  is expected to use, less what the figure already shows of it. Without this every held job would
  start in the second the figure dips under the mark. The expectation is the median of CPU seconds
  per second of run time of the same command in the same repository (at least 3 runs of a second
  or longer among its last 20), else 1 CPU.
- **Held jobs ahead.** A held job counts the held jobs that are served before it, so they start
  in the queue's order, by level and then by arrival.

A held job holds no slot. Its client prints the reason once, and `slot status` shows it under
the job:

```text
slot: job 12 waiting: host busy: pool 12.0 + compile cache 6.0 of 20 CPUs, mark 17.0
slot: job 13 waiting: host busy: pool 9.5 + compile cache 5.0 of 20 CPUs + 3.0 for jobs that just started, mark 17.0
```

While the limit is on, `slot status` has the line `starts held while the pool and the compile cache use more than 17.0
CPUs and 4 jobs run`. The check runs every 2 seconds and once more when the job has its slots, so
a change of the mark reaches the queued jobs within 2 seconds, without a reload.

No job waits for ever on this check: a job that has been held for the host's CPUs for 10 minutes
starts, whatever the figure says. Its client prints `slot: job 12 was held 10m for the host's
CPUs, as long as a job is: it starts`, and the census log gets a `cpu_hold_bound` line beside the
`cpu_hold` line of the job's first hold. The 10 minutes start again when the daemon reloads.

A job whose command is predicted to run 30 seconds or less (the 90th percentile of its last runs
in the same repository, with at least 3) is not held by this check: it adds little load for a
short time. Its client prints `slot: job 12 is predicted to run 4 s: it is not held for the
host's CPUs`, and the census log gets a `cpu_hold_short` line. A command without history is held
as any other.

A mark above the host's CPU count never holds a start for CPU time (the pressure mark below still
holds), and `slot cpu-limit` says so. A mark under 1 is refused. For the first 5 seconds after its start the daemon has no figure yet and holds
every start past the floor, so a reload does not start every held job at once. After that, a
daemon whose last sample is older than 5 seconds holds nothing.

**The pressure mark.** On a saturated host the pool's CPU time cannot rise past its share, however
many threads wait, so the figure above stops rising while the host keeps getting slower. CPU
pressure keeps rising: `some avg10` of `/proc/pressure/cpu`, the share of the last 10 seconds in
which a runnable task waited for a CPU, in percent. `slot cpu-limit 17 --pressure 80` also holds a
queued job while that value is at 80 % or more and the floor's jobs run, and lets it start when the
value falls under 80; the reason reads `host busy: CPU pressure 85 % at or over the mark 80 %`.
The two marks hold independently, and the pressure mark holds even with a CPU mark above the
host's CPU count. It needs a CPU mark: `--pressure` without one is refused, and `slot cpu-limit off`
clears both. `--pressure off` forgets it; it is off until one is named. The bound of 10 minutes,
the pacing of the jobs that reach it and the pass for jobs predicted short apply to it as above.
In the API it is `cpu_pressure_max` (a percent over 0, at most 100; `null` turns it off), and `GET
/api/cpu-limit` answers the current value as `pressure`. `slot cpu-limit` prints both marks and both
current values, and `/api/state` has `cpu_pressure_max` while it is set.

What it costs: with the limit on, a queue forms while the host is saturated, also when slots are
free. Work outside the pool and the compile cache does not hold a job, but it shares the CPUs
with the jobs that start.

The mark and the floor are kept in `limits.json` beside the headroom and hold across reloads and
restarts. `GET` and `PUT /api/cpu-limit` read and change them (`{"cpu_busy_max": 17,
"cpu_busy_floor": 4}`, either field alone; `"cpu_busy_max": null` switches the limit off; a mark
under 1, a floor under 1, a floor without a mark and a body with neither field are refused with
400). `GET` also answers the figure's parts, `pool`, `compile_cache` and `outside` in CPUs, and
`busy`, the whole host. `/api/state` has `cpu_busy_max` and `cpu_busy_floor` while the limit is
on.

## Admission by share

A slot promises one CPU, and many jobs do not use it: a test that waits on a socket holds its
slot at 0 CPU, and a workspace build that asked for 4 slots runs a dozen compilers in the compile
cache server. `slot admission shares` admits jobs by the CPUs they use instead. It is off on a
fresh install (`slots`), and `slots` is exactly the behaviour without it.

```sh
slot admission                       # print the setting and, by share, the budget and the free share
slot admission shares                # admit by share
slot admission shares --floor 2      # and let 2 jobs run whatever the share says
slot admission --max 30 --pressure 70
slot admission --min-budget 8        # the budget never falls under 8 CPUs
slot admission slots                 # back to slots
```

**A job's share** is the CPUs its command used before: CPU seconds per second of run time of the
last 20 runs of the same command in the same repository, newer runs weighing more, the 90th
percentile plus 15 %, at least 0.1. The compilers a compile cache server ran for the job count.
A command with fewer than 3 runs of a second or longer asks for 1 CPU. `slot --tokens N` (and a
client of 0.21.0 or older that asks for slots) asks for N CPUs, at most the budget; a larger
request is told the share it gets (`slot: --tokens 16 capped at a share of 15.0 CPUs, the
budget`).

**A queued job starts** in the queue's usual order (level, then the jobs that may no longer be
passed, short jobs, the rest, then arrival) while

- fewer than `--max` jobs run (48 until one is named), and
- fewer than `--floor` jobs run (4 until one is named), or its share fits in the free share.

The free share is the budget less the CPUs the pool and the compile cache used over the last 5
seconds, less what the jobs that started in those 5 seconds are expected to add. The daemon looks
once a second, so a job starts within a second of its share coming free. A running job that uses
almost no CPU (under 0.1 CPU for 2 seconds) counts with what it uses, not with its share: the
queue gets the rest, and the job's record carries the CPU-seconds it lent (`lent_s`). When it
uses CPU again, that CPU enters the figure at a fifth per second (the figure is a 5-second mean),
and no further job starts until the free share covers one. Until the daemon has a CPU figure
(right after a start or a reload) every running job counts with its whole share.

**The budget** starts at three quarters of the host's CPUs and is never above the pool's CPU
quota, or the host's CPUs less the headroom. Every 5 seconds it grows by 0.5 CPU while the jobs
that ended ran at most 1.3 times as long as their best earlier run (the median of them), and
shrinks to 85 % when they ran more than twice as long. The best earlier run is one that ended ok
and was not served from the compile cache. The budget never falls under `--min-budget` (half the
host's CPUs until one is named), nor under the shares of the floor's jobs. This controller has
not yet been measured with shares on: try it on one host before you switch it on elsewhere.

**Over a CPU pressure** of `--pressure` (60 % until one is named; `/proc/pressure/cpu`) only jobs
predicted to run 5 seconds or less start, beside the floor. The admission by disk and memory
holds jobs as by slots.

A job let through by share holds no slot of its own. The slots in circulation are at most the
budget's CPUs, so build tools that read the jobserver run at most one process per running job
(the implicit slot every jobserver client has) plus one per CPU of the budget. Back by slots, a
job that started by share keeps one slot held back until it ends, and `slot usage` charges it by
the admission it started under.

A queued job's line says what it waits for:

```text
slot: job 14 waiting: waits for a share of 4.0 CPUs: 3.0 of a budget of 15.0 free
```

By share `slot status` has the line `shares: budget 15.0 of 17.0 CPUs, 3.2 free (9.0 busy, 2.8
for jobs that just started), 12 running (4 to 48), 2 idle lend 2.0`, and each job its share and
what it uses (`share 2.3, uses 0.4`). The web view names the slots column `share` and shows
`used/asked`, and the free share under the header. `slot usage` charges a job its share for its
run, less what it lent.

The setting is kept in `limits.json` beside the headroom and holds across reloads and restarts;
the daemon applies it before it takes or resumes any job. `GET` and `PUT /api/admission` read and
change it (`{"admission": "shares", "floor": 4, "max": 48, "pressure_max": 60, "min_budget":
10}`, any field alone; a floor or most under 1, a floor over the most, a pressure outside 0 to
100 and a lower bound under 1 CPU are refused with 400). `/api/state` and every tick have
`shares` while it is on. A daemon older than the setting answers 404 on `/api/admission`, for
every method.

### Not built yet

A CPU weight per job (a cgroup of its own per job) needs the service's cgroup to be delegated,
which needs root.

## Freeing disk space

The daemon also frees disk. Every minute it looks at the filesystems of the Cargo target
directories jobs built into. When one has less than 40 GB free (`slot serve --evict-below`, in
GB; 0 turns it off), it deletes target directories there, least recently built first, until the
filesystem would have 50 GB free.

On a build server the same rules apply to the target directories of the jobs sent to it: a
finished job that arrived with its tree makes the directory it built into known. A target
directory under the daemon's cache root is left out; the cache keeps to its own size limit.
When the daemon knows no target directory that exists, it measured nothing, and `slot status`
says so instead of naming free space:

```text
disk: no target dir known, nothing measured (eviction below 40.0 GB)
```

Eviction starts no later than the wait for disk above: its mark is the larger of `--evict-below`
and `--min-free-disk`. With `--min-free-disk 50` it deletes below 50 GB, until 60 GB would be
free, and `slot status` says the mark was raised:

```text
disk: 45.0 GB free, under 50.0 GB, raised from 40.0 GB to the disk hold's mark: deleted 2 target dir(s), 21.4 GB
```

A job that starts to wait for disk makes the daemon look at once instead of at the next minute.
`--evict-below 0` still turns eviction off.

`slot evict-below` changes the mark on the running daemon, without a restart and without
changing the wait for disk:

```sh
slot evict-below            # the mark and the idle keep in force, and where they come from
slot evict-below 100        # delete below 100 GB free; the daemon looks at once
slot evict-below 0          # eviction off
slot evict-below default    # forget both: --evict-below and 2 hours apply again
slot evict-below 100 --idle 30m   # also delete target dirs idle for 30 minutes
```

The mark set this way replaces `--evict-below`, is still raised to `--min-free-disk` when that is
larger, and is kept in `limits.json` in the state directory across reloads and restarts. The
daemon serves it at `GET` and `PUT /api/evict-below` (`{"evict_below_gb": 100}`, `null` to
forget it).

`--idle` sets how long eviction keeps a target directory after its last build: from `5m` to
`168h` (7 days), 2 hours until it is set. After a host restart every session rebuilds, and with
2 hours nearly every target directory is too new to delete. It applies at the next look, is kept
in `limits.json` like the mark, and is `evict_idle_s` (seconds) in the same `PUT` body; a field
left out stays as it is, `null` forgets it. Eviction never deletes:

- a target directory a queued or running job builds into, or one equal to, inside or holding the
  one its `CARGO_TARGET_DIR` names;
- one in the tree of a queued or running job: git's top level of its working directory, else the
  working directory, with nested ones such as `target/ignite-test/rt` or `driver/target`; another
  worktree or clone of the same repository may go;
- one in the tree of any job that ran in the last hour, so the jobs of one long task keep their
  target directories between them;
- one built in the last 2 hours, or the idle keep set with `--idle` (never less than an hour);
- one with a `.cargo-lock` anywhere in it that a process holds locked;
- one whose deletion failed in the last hour, for example a managed worktree with a live session
  lease: the next look deletes the next candidates instead (the journal says
  `slot: disk: eviction leaves <path> out for 1h: its removal failed` once);
- a directory no job built into, or one without the `CACHEDIR.TAG` Cargo writes;
- a directory a process uses: its executable, its working directory, or a file it has open or
  mapped is inside it;
- a directory a systemd user unit starts from: an `ExecStart` line of a unit file or drop-in in
  `~/.config/systemd/user` (or the runtime directory's `systemd/user` and `systemd/transient`)
  names a path inside it, whether or not the unit runs.

A directory kept for a process or a unit gets a journal line, `slot: disk: kept <path>: process
1234 runs it`.

A target directory inside a managed worktree is deleted with `worktree discard-cache`.

Eviction deletes by default. To only see what it would delete, run it as a dry run: either
`slot serve --evict-dry-run true`, or a file `evict.json` in the daemon's state directory
(`~/.local/state/slot` by default), which a provisioning tool can write without touching the
service's command line:

```json
{ "dry_run": true }
```

The daemon reads the file at every look, so a change applies within a minute without a reload;
when the file sets `dry_run`, it wins over the flag. Under the floor, a dry run says in the journal
and in `slot status` that eviction is off and what it would free, then lists the directories:

```text
disk: 35.2 GB free, under 40.0 GB: eviction is off (dry run), would free 21.4 GB in 3 dir(s)
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

A rust job's record also has `cache_env`, the variables that decide what the compile cache can
do, and `warnings` when they switch the cache off or name an sccache server of its own: see
[Environment variables](reference/environment.md#recorded-for-a-rust-job).

The record says where a job's time went: a cargo `test`, `nextest` or `bench` job's compile and
test time (`compile_ms`, `test_ms`), a remote job's sync of its tree (`sync_ms`, `sync_bytes`)
and copy of its products and fetches back (`back_ms`), and a remote cargo job's target directory
on the node and what that held when the job queued (`node_target_dir`, `seeded_from`): see
[Environment variables](reference/environment.md#recorded-phases-of-a-job).

The daemon reads the log back at start, so the web UI shows recent jobs across restarts.

`project` is the namespace of the repository's git remote: every path segment of the origin URL
but the last, so `git@forge.example:team-a/api.git` and `git@forge.example:team-a/web.git` are both
project `team-a`, and so is every worktree of them. `project = "web"` in the repository's
`.slot.toml` names the project instead. A job without a remote has no `project` field.

## Usage

`slot usage` shows how the pool was shared over a window, from the job log:

```text
$ slot usage --since 24h
4789 jobs, 110.9 slot-h, 89.4 cpu-h, by project
project                       jobs  slot-h  share  cpu-h  share wait p50
team-a                        1338    40.6  36.6%   23.1  25.8%      46s
team-b                         189    26.2  23.6%   21.1  23.6%      60s
```

- **slot time** is run time × slots held, clipped to the window: the share of the pool a group took.
- **CPU time** is the measured CPU of the group's jobs. It can be far below the slot time when
  jobs hold slots while they wait on something else (a cluster, a network call).
- **wait p50** is the median queue wait of the group's jobs.

`--by project` (default), `--by repo`, `--by session` or `--by node` (the build node a job ran
on: `local` for this host's own); `--since 90m`, `24h` (default) or `7d`;
`--json` for the whole answer. A job logged before jobs carried a project is grouped by the
project of its directory now, or by its repository name when the directory is gone. The same data
is `GET /api/usage?from=<ms>&to=<ms>&by=project` and the **Share of the pool** table in the web UI.

The jobs the build nodes of the host configuration file ran count too: when a poll shows that a
node's job ended, the daemon appends it once to `<state-dir>/node-jobs.jsonl`, with `node` set
to the node's name and the id the node gave it ([Remote builds](remote-builds.md#one-registry-of-build-nodes)).
A job this daemon handed to a node counts once, by this host's record of it: the node's row of
it (same node, command and directory, started there while it ran here) is left out.

## Web UI

The job tables show the Git repository that owns the working directory.
A linked worktree also shows its directory name, including for detached HEAD.
A build outside Git shows **No repository**. Jobs from older clients keep their
recorded names with an **Unverified** label because the name may identify a directory.
An older job with an empty name shows **Repository unknown**.

During a daemon handover, Slot passes the last 900 chart samples and recent job output to the next process.
An older sender can only pass the data its version captured. The job log survives either upgrade.

The CPU and queue chart also reads up to six hours of ten-second averages from
the census log. Select **6 h** to see the full window. Historical samples show
mean host CPU in cores and mean queued jobs. The toolchain breakdown is available
during periods covered by the one-second live samples.

The daemon serves a web UI on `127.0.0.1:7411` (`--http <address>`, `--http off` to disable). Its
start page is the cluster view: the headline numbers (running and queued jobs, the slot wait of 9
in 10 jobs, CPUs in use of those available, the compile cache's share of the last 24 hours, and
the cost of remote machines once one runs, from each machine's boot to now), the queue (when it
is empty: each node's running jobs and busy slots), a card per node (slots by toolchain, CPU over
60 s of the whole machine with its load, the jobs it finished in the last 60 minutes and today as
the node counts them, failures by cause, memory, free disk, the checkouts with jobs on it, the time
left of a remote node's lease: to the poweroff the node has scheduled, the card red when that lies
more than 5 minutes from the deadline of the cluster file) with each job moving from the queue to
its node, and the newest jobs with the node each runs on and its session. Selecting a node card, or
Enter on it, shows only that node's jobs; Escape shows all again. Below 1100 px the page stacks:
the queue above the node cards, the job list below them. The page reads `GET /api/nodes`, one row
per node (today the local machine). **Jobs and
charts** (`#jobs`) opens the view of the job tables, which shows:

- slots in use on this host out of its pool, and the **headroom for you** slider with buttons for 0, 20 and 40 %
  ([Headroom](headroom.md)); on the machine base, the line with the CPUs busy on the host, the
  target, the pool's quota and the busiest owners outside the pool; when a job failed in the last
  60 minutes, a **failed** count over every node, split by cause (test; timeout, exit 124;
  infra: a lost connection, exit 3, or a command that could not run or was not found, 125 or 127),
  that opens the output of the newest one,
- in the row of line charts, **Compile cache**: for each minute, the share of the compiler calls
  that sccache answered from its cache, of all calls and of the cacheable ones (hits and misses),
  read from `GET /api/cache-history`; the legend gives the newest minute's share and, after
  `24 h:`, the share of the last 24 hours, read from `GET /api/cache-day`; the daemon computes
  both from the lines of `cache.jsonl` ([sccache watchdog](#sccache-watchdog)),
- directly under the header, queued and running jobs: repository, command, state, wait, elapsed
  time, cores, memory, session (its title, else the first 8 characters of its id), and for a queued
  job its level; a job of a build node names the node under its job number; a running job's row
  opens on what it compiles now; cores and memory are measured only for jobs on this host,
- the finished jobs that ended last, a build node's among them: result (`ok`, `failed (101)` with the exit code, `cancelled`), wait, duration,
  CPU seconds, average cores, peak memory,
- build cores on the whole host by tool family and by agent session, run by slot or **not on
  slot**,
- a timeline with one bar per job, coloured by toolchain; the pale part of a bar is the wait for
  a slot,
- cores in use by slot jobs per toolchain, and by the whole host,
- the share of this host's pool per project, repository or session for today, the last 24 hours or the
  last 7 days ([Usage](#usage)),
- the live output of a job, opened by clicking its bar or its job number in a table; the job number
  is a button, so the keyboard reaches it with Tab. Opened by the keyboard, the output takes the
  focus; Escape closes it and puts the focus back on the job.

It keeps the last 500 finished jobs in memory (`--keep-jobs`). The UI has no authentication;
keep it on a loopback address. The same data is available as JSON from `/api/state`, as a stream
of server-sent events from `/api/events`, and per job from `/api/jobs/<id>/output`. `/api/nodes`
answers an array with one row per node: `node_id` (`local` for the daemon's machine), `kind`,
`state`, `cpus`, `slots`, `slots_in_use`, `running_jobs`, `queued_jobs`, `memory_gib`,
`memory_used_mib`, `disk_gib`, `disk_free_gib`, `cpu_busy_milli` (the CPUs of the pool and the
compile cache over the last 10 s, in thousandths), `host_cpu_milli` and `load1` (the whole
machine, every program counted), `booted_at`, `poweroff_at` (the poweroff systemd has scheduled,
from `/run/systemd/shutdown/scheduled`), `ended_hour` and `ended_hours` (the jobs the node ended in
the last 60 minutes and per clock hour of the last 25, as `ok`, `test`, `timeout`, `infra` and
`cancelled`, from its own job log) and `joined_at`; a remote node adds
`machine_type`, `location`, `price_per_hour_cents` and `deadline`. Every job in `/api/state` names
its `node`. `/api/limits`
reads and sets the headroom, `/api/priority` the [priority rules](#priority).
