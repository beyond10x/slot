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

The first job in that order takes slots as they come free and keeps them until it has all it
asked for: a job that needs 4 slots is not overtaken by later 1-slot jobs of its level or below,
even while one of them would fit. A later job of a higher level does overtake it, and takes the
slots it gathered; after 30 minutes the waiting job is `high` itself.

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

`slot serve --sccache-watch false` turns the watchdog and the counter log off.

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

A cargo build (`cargo build`, `check`, `clippy`, `test` and the like) also waits, without a
slot, while another job of the pool compiles in the same target directory: cargo would only
print `Blocking waiting for file lock on build directory` there. Its reason reads
`target directory in use by job 12`; it goes on when that job has compiled (cargo runs tests
without the lock) or ended, and the jobs behind it in the queue start meanwhile.

## Freeing disk space

The daemon also frees disk. Every minute it looks at the filesystems of the Cargo target
directories jobs built into. When one has less than 40 GB free (`slot serve --evict-below`, in
GB; 0 turns it off), it deletes target directories there, least recently built first, until the
filesystem would have 50 GB free. It never deletes:

- a target directory of a repository with a queued or running job;
- one built in the last 2 hours;
- a directory no job built into, or one without the `CACHEDIR.TAG` Cargo writes.

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

`--by project` (default), `--by repo` or `--by session`; `--since 90m`, `24h` (default) or `7d`;
`--json` for the whole answer. A job logged before jobs carried a project is grouped by the
project of its directory now, or by its repository name when the directory is gone. The same data
is `GET /api/usage?from=<ms>&to=<ms>&by=project` and the **Share of the pool** table in the web UI.

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

The daemon serves a web UI on `127.0.0.1:7411` (`--http <address>`, `--http off` to disable). It
shows:

- slots in use out of the pool, and the **headroom for you** slider with buttons for 0, 20 and 40 %
  ([Headroom](headroom.md)); on the machine base, the line with the CPUs busy on the host, the
  target, the pool's quota and the busiest owners outside the pool; when a job failed in the last
  60 minutes, a **failed** count that opens the output of the newest one,
- in the row of line charts, **Compile cache**: for each minute, the share of the compiler calls
  that sccache answered from its cache, of all calls and of the cacheable ones (hits and misses),
  read from `GET /api/cache-history`, which the daemon computes from the lines of `cache.jsonl`
  ([sccache watchdog](#sccache-watchdog)),
- directly under the header, queued and running jobs: repository, command, state, wait, elapsed
  time, cores, memory, session, and for a queued job its level; a running job's row opens on what
  it compiles now,
- finished jobs: result (`ok`, `failed (101)` with the exit code, `cancelled`), wait, duration,
  CPU seconds, average cores, peak memory,
- build cores on the whole host by tool family and by agent session, run by slot or **not on
  slot**,
- a timeline with one bar per job, coloured by toolchain; the pale part of a bar is the wait for
  a slot,
- cores in use by slot jobs per toolchain, and by the whole host,
- the share of the pool per project, repository or session for today, the last 24 hours or the
  last 7 days ([Usage](#usage)),
- the live output of a job, opened by clicking its bar or its job number in a table; the job number
  is a button, so the keyboard reaches it with Tab. Opened by the keyboard, the output takes the
  focus; Escape closes it and puts the focus back on the job.

It keeps the last 500 finished jobs in memory (`--keep-jobs`). The UI has no authentication;
keep it on a loopback address. The same data is available as JSON from `/api/state`, as a stream
of server-sent events from `/api/events`, and per job from `/api/jobs/<id>/output`. `/api/limits`
reads and sets the headroom, `/api/priority` the [priority rules](#priority).
