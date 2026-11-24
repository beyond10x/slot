---
title: Environment variables
description: Variables slot reads, variables it sets in a job, and variables it drops.
---

# Environment variables

## Read by the client

| variable | default | effect |
|---|---|---|
| `SLOT_HOST` | unset | ssh host of a remote build server; unset means the daemon on this machine. See [Remote builds](../remote-builds.md). |
| `SLOT_SSH` | `ssh -o ControlMaster=auto -o ControlPersist=10m -o ControlPath=$XDG_RUNTIME_DIR/slot-ssh-%C -o ConnectTimeout=3` | ssh command for rsync and the run, split on whitespace |
| `SLOT_WSROOT` | `.local/state/slot/ws` | workspace root on the remote host, relative to its home directory; must match the server's `--ws-root` |
| `SLOT_REMOTE` | `slot` | the slot executable on the remote host |
| `SLOT_ENV` | unset | extra variable names forwarded to a remote host, comma-separated |
| `SLOT_SYNC_STALL` | `60s` | a remote sync that reads and writes no byte for this long is killed and the job runs on this machine's daemon: seconds, or a number with `s`, `m` or `h`. See [Remote builds](../remote-builds.md#when-the-server-is-unreachable). |
| `SLOT_SYNC_LIMIT` | `5m` | a remote sync that runs longer than this is killed and the job runs on this machine's daemon; same format |
| `SLOT_SOCKET` | `slot.sock` in `$XDG_RUNTIME_DIR`, else in `/run/user/<uid>` | the local daemon's socket |
| `SLOT_LOCAL_FALLBACK` | unset | `1`: with no daemon answering, run the command here at once, outside the pool. Unset or `0`: wait up to 60 s for the socket, then exit with 125 without running it. See [Getting started](../getting-started.md#when-the-daemon-is-not-running). |
| `SLOT_TOKENS` | unset | slots a job holds, as `slot --tokens N` (which wins over it); capped at the slots in circulation, ignored with `--local`; by share (`slot admission shares`) the job asks for a share of N CPUs, at most the budget, and holds no slot. See [Toolchains](../toolchains.md#naming-the-slots) and [Admission by share](../how-it-works.md#admission-by-share). |
| `SLOT_PRIORITY` | unset (`normal`) | a job's queue level, `urgent`, `high`, `normal` or `low`, as `slot --priority` (which wins over it); a rule set with `slot priority set` for the repository wins over both; ignored with `--local`. See [Priority](../how-it-works.md#priority). |
| `SLOT_TIMEOUT` | unset | how long a job may run before the daemon ends it, in place of the daemon's run-time limit: seconds, or a number with `s`, `m` or `h` (`90s`, `30m`, `1h`), as `slot --timeout` (which wins over it); capped at the most a job may name (1 h unless `slot run-limit --max` changed it); ignored with `--local`. See [Run-time limit](../how-it-works.md#run-time-limit). |
| `SLOT_GATE` | unset | a gate directory `slot gate` serves: the client queues through its FIFOs instead of the socket and runs the command itself, for a sandbox that cannot reach the socket. See [Agent integration](../agent-integration.md#builds-in-a-sandbox-the-gate). |
| `SLOT_HTTP` | `127.0.0.1:7411` | the daemon's web address for `slot status`, `slot headroom`, `slot priority`, `slot run-limit`, `slot job-parallel`, `slot cpu-limit`, `slot admission` and `slot sessions` |
| `SLOT_HOOK` | `block` | mode of `slot hook claude-pre-tool-use`: `block` refuses a build outside the pool, `warn` only adds advice, `off` allows everything. See [Claude Code plugin](../claude-code-plugin.md). |

## Read by the daemon

| variable | effect |
|---|---|
| `SLOT_SOCKET` | default for `slot serve --socket` and `slot proxy --socket` |
| `SLOT_GATE_DIRS` | default for `slot gate --dir`, directories separated by `:` |
| `XDG_RUNTIME_DIR` | directory of the default socket; without it `/run/user/<uid>` when that exists, else `slot-<uid>.sock` in the temporary directory |
| `XDG_STATE_HOME` | the default state directory is `$XDG_STATE_HOME/slot`, otherwise `~/.local/state/slot` |

## Set in every job

| variable | value |
|---|---|
| `SLOT_JOB` | the job id, used to attribute sccache CPU time to the job |
| `MAKEFLAGS`, `CARGO_MAKEFLAGS` | `-j --jobserver-auth=fifo:<state-dir>/jobserver.fifo`, for rust and other jobs; `fifo:<gate-dir>/jobserver.fifo` for a gated job; not set this way while `slot job-parallel` is on |
| `CARGO_BUILD_JOBS`, `MAKEFLAGS` | only while [`slot job-parallel N`](../how-it-works.md#build-processes-per-job) is on, for every job: `<P>` and `-j<P>`, where P is N or the slots the job holds if those are more; `CARGO_MAKEFLAGS` and `MFLAGS` are then not set, and nothing names the pool's FIFO |
| `RUST_TEST_THREADS`, `NEXTEST_TEST_THREADS` | `slot serve --test-threads` (default a quarter of the pool, at least 2); rust and other jobs, unless the caller set them |
| `GOMAXPROCS` | slots held for go jobs; `--test-threads` for other jobs; unless the caller set it |
| `GOFLAGS` | the caller's value plus `-p=<slots held>`; go jobs |
| `CARGO_TARGET_DIR` | `<cache-root>/target/<workspace-id>`; remote rust and other jobs |
| `GOCACHE` | `<cache-root>/go-build`; remote go and other jobs |

## Dropped from the caller's environment

`MAKEFLAGS`, `MFLAGS`, `CARGO_MAKEFLAGS` and `CARGO_BUILD_JOBS`. Each would let a job bypass the
pool. The caller's values are dropped with `slot job-parallel` on too: the daemon then sets
`CARGO_BUILD_JOBS` and `MAKEFLAGS` itself.

A local job otherwise gets the caller's full environment. A remote job starts from the server
daemon's environment, plus the variables its [toolchain](../toolchains.md) admits and the names
in `SLOT_ENV`. A name that looks like a credential (it contains `TOKEN`, `SECRET`, `PASSWORD`,
`PASSWD`, `CREDENTIAL`, `_KEY` or `AUTH`) crosses only when `SLOT_ENV` names it.

## Open-file limit

A local job runs with the open-file limit (`ulimit -n`, soft and hard) of the process that
called `slot`, not with the daemon's: a daemon started by systemd has a soft limit of 1,024,
a login shell often far more. The client sends its limit with the request. Where the caller's
limit is above the daemon's hard limit, the job gets the daemon's hard limit, and the client
prints one line:

```text
slot: warning: open files: the caller's limit is 2000000, the daemon's hard limit is 1000000: the job gets 1000000
```

The job has its limit from its first instruction on: when it differs from the daemon's, the
daemon starts the job through the slot binary itself, which sets the limit in the job's
process and then executes the job's command in its place (same process, same process group).
The job's record, `slot status` and the web view show the job's own command. The daemon's
own limit does not change, so a request without a limit (a client older than this feature)
runs with the limit the daemon was started with, also after a reload.

In remote mode the build server's limit applies: the client does not send its own, and the
daemon there runs the job with its own limit. A job behind [the gate](../agent-integration.md#builds-in-a-sandbox-the-gate)
runs in the sandbox with the sandbox's limit.

Not covered: the other limits, among them the stack size (`ulimit -s`), the core file size and
the number of processes. A job gets the daemon's values of those.

## Recorded for a rust job

The record of a rust job in the [job log](../how-it-works.md#job-log) has a `cache_env` map with
these variables as the job got them, when they are set: `RUSTC_WRAPPER`,
`CARGO_BUILD_RUSTC_WRAPPER`, `CARGO_INCREMENTAL`, `RUSTFLAGS`, `CARGO_ENCODED_RUSTFLAGS`,
`SCCACHE_SERVER_PORT`, `SCCACHE_DIR`, `RUSTUP_TOOLCHAIN` and every `CARGO_PROFILE_*`. No other
variable is recorded. An empty value is recorded as an empty string; a value longer than 200
characters is cut and ends in `…`. sccache's cache key holds the compiler flags and every
`CARGO_*` variable, so these values say whether the job could be answered from the cache. Three
values give a warning: the client prints `slot: warning: <text>` on stderr when the job starts,
`slot status` lists it under the job, and the record has it in `warnings`. The job runs
unchanged.

| warning | when |
|---|---|
| `compile cache off: RUSTC_WRAPPER is empty` | `RUSTC_WRAPPER` is set and empty; with `RUSTC_WRAPPER` unset and `CARGO_BUILD_RUSTC_WRAPPER` empty, the warning names that variable |
| `private sccache server (port N)` | `SCCACHE_SERVER_PORT` is a port other than 4226 |
| `private sccache directory` | `SCCACHE_DIR` is set |
