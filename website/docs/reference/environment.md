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
| `SLOT_REMOTE_FALLBACK` | unset | `local`: a remote job that cannot start runs on this machine's daemon instead; unset: it does not run and `slot` exits with 3. See [Remote builds](../remote-builds.md#when-the-server-is-unreachable). |
| `SLOT_SYNC_STALL` | `60s` | a remote sync that reads and writes no byte for this long is killed and the job did not start (see `SLOT_REMOTE_FALLBACK`): seconds, or a number with `s`, `m` or `h`. See [Remote builds](../remote-builds.md#when-the-server-is-unreachable). |
| `SLOT_SYNC_LIMIT` | `5m` | a remote sync that runs longer than this is killed and the job did not start; same format |
| `SLOT_NODE_WAIT` | `35m` | how long a remote job waits for a server whose daemon does not answer (it restarts, or lets its jobs end before it does); the job is sent again every 5 s, and after this long it did not start (see `SLOT_REMOTE_FALLBACK`). Same format; `0`: no wait, the call ends at once with exit 3. A job the daemon handed to a node does not wait, whatever the value: it is queued here again. See [Remote builds](../remote-builds.md#when-the-servers-daemon-does-not-answer). |
| `SLOT_REMOTE_SAME_PATH` | unset | `1`: mirror the repository at its own absolute path on the remote host and run the job there; the server needs `--same-path-root` (see [Remote builds](../remote-builds.md#mirror-at-the-same-path)) |
| `SLOT_REMOTE_SAME_PATH_ROOT` | `$HOME` | with `SLOT_REMOTE_SAME_PATH`, only a repository below this directory keeps its own path on the remote host; any other goes to the remote workspace root (`SLOT_WSROOT`) |
| `SLOT_NO_PRODUCTS` | unset | `1`: a remote `cargo build`, `test`, `bench` or `rustc` copies nothing back into the local target directory, and a remote `go build -o` or `go test -c -o` leaves nothing at the path `-o` names; the command runs on the remote host unchanged, and the client prints `slot: artifacts not fetched: SLOT_NO_PRODUCTS is set` where it would have copied. Unset or `0`: the products come back. See [Remote builds](../remote-builds.md#binaries-and-test-binaries-come-back). |
| `SLOT_SOCKET` | `slot.sock` in `$XDG_RUNTIME_DIR`, else in `/run/user/<uid>` | the local daemon's socket |
| `SLOT_PHASES_FILE` | unset | in remote mode, after the job: the file the client writes the job's sync, back time, node target directory and seed to, as JSON. The daemon sets it for a job it hands to a node; a caller has no need to. See [Recorded phases of a job](#recorded-phases-of-a-job). |
| `SLOT_SPILLED` | unset | set by the daemon in the client of a job it hands to a node: a remote start that fails ends with exit 3 whatever `SLOT_REMOTE_FALLBACK` says, without the wait of `SLOT_NODE_WAIT`, and the daemon queues the job here again. In a caller's environment it does the same to a failed start, with no daemon to queue the job again. A job a deny list keeps on this machine ends with exit 3 only in the daemon's own client (`SLOT_SPILLED` with `SLOT_PHASES_FILE`); with `SLOT_SPILLED` alone it runs in this machine's pool. See [Remote builds](../remote-builds.md#trees-that-never-leave-this-machine). |
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
| `SLOT_CLUSTER_FILE` | default for `slot serve --cluster-file`: the host configuration file whose `[remote.<name>]` tables are the nodes the daemon polls; `off` reads none |
| `XDG_CONFIG_HOME` | the default host configuration file is `$XDG_CONFIG_HOME/slot/cluster.toml`, otherwise `~/.config/slot/cluster.toml` |

## Set in every job

| variable | value |
|---|---|
| `SLOT_JOB` | the job id, used to attribute sccache CPU time to the job |
| `SLOT_SANDBOX`, `TMPDIR` | `bwrap` and `/tmp` (a private tmpfs), for every job of a daemon started with [`slot serve --sandbox`](../remote-builds.md#run-every-job-in-a-sandbox) |
| `MAKEFLAGS`, `CARGO_MAKEFLAGS` | `-j --jobserver-auth=fifo:<state-dir>/jobserver.fifo`, for rust and other jobs; `fifo:<gate-dir>/jobserver.fifo` for a gated job; not set this way while `slot job-parallel` is on |
| `CARGO_BUILD_JOBS`, `MAKEFLAGS` | only while [`slot job-parallel N`](../how-it-works.md#build-processes-per-job) is on, for every job: `<P>` and `-j<P>`, where P is N or the slots the job holds if those are more, and for a remote job its width (`SLOT_WIDTH`) if that is more; `CARGO_MAKEFLAGS` and `MFLAGS` are then not set, and nothing names the pool's FIFO |
| `SLOT_WIDTH` | for every remote job (synced from another host, so run on this machine as a node): its width, set once when it starts: the CPUs free at that moment (the host's CPUs less the busy ones, the mean of the last seconds) divided by the jobs that started in the last 5 s, this one counted, at least 1 and at most the host's CPUs. A Playwright or Vitest config reads it for its worker count (`workers: Number(process.env.SLOT_WIDTH ?? 2)`). The job's record carries it as `width`. Not set for a local job |
| `RUST_TEST_THREADS`, `NEXTEST_TEST_THREADS` | `slot serve --test-threads` (default a quarter of the pool, at least 2) for local rust and other jobs, unless the caller set them; the width for a remote job, and a lower count the caller set is raised to it |
| `GOMAXPROCS` | slots held for local go jobs; `--test-threads` for local other jobs; unless the caller set it. For a remote job the width (go jobs: or the slots held if more), and a lower count the caller set is raised to it |
| `GOFLAGS` | the caller's value plus `-p=<slots held>`; go jobs; for a remote go job `-p=<width>`, or the slots held if more |
| `CARGO_TARGET_DIR` | for remote rust and other jobs. A tree mirrored below the workspace root: `<cache-root>/target/<workspace-id>`. A tree [mirrored at the same path](../remote-builds.md#mirror-at-the-same-path): not set, and removed from the server daemon's own environment together with `CARGO_BUILD_TARGET_DIR`, when the job builds into the directory cargo resolves by itself (`target` beside the workspace's root manifest); the caller's directory as an absolute path when the caller set `CARGO_TARGET_DIR`, when `CARGO_BUILD_TARGET_DIR` or a `build.target-dir` of a cargo configuration on the caller's machine names another directory, or when `cargo metadata` failed on the caller's machine (then `target` beside the nearest `Cargo.lock`). With [`--sandbox-fixed-paths`](../remote-builds.md#run-every-job-in-a-sandbox): `/work/target`. Never set for a local job |
| `GOCACHE` | `<cache-root>/go-build`; remote go and other jobs |
| `SLOT_PHASES_FILE` | `<state-dir>/phases-<id>.json`, for a job the daemon hands to a node: the remote-mode client that runs it writes what it measured there, and the daemon adds that to the job's record when the job ends (see [below](#recorded-phases-of-a-job)) |

## Dropped from the caller's environment

`MAKEFLAGS`, `MFLAGS`, `CARGO_MAKEFLAGS` and `CARGO_BUILD_JOBS`. Each would let a job bypass the
pool. The caller's values are dropped with `slot job-parallel` on too: the daemon then sets
`CARGO_BUILD_JOBS` and `MAKEFLAGS` itself.

A local job otherwise gets the caller's full environment. A remote job starts from the server
daemon's environment, plus the variables its [toolchain](../toolchains.md) admits and the names
in `SLOT_ENV`. A name that looks like a credential (it contains `TOKEN`, `SECRET`, `PASSWORD`,
`PASSWD`, `CREDENTIAL`, `_KEY` or `AUTH`) crosses only when `SLOT_ENV` names it.

## Cargo variables and the compile cache

A compile cache such as sccache puts every `CARGO_*` variable of the compiler's environment into
the key of a Rust unit, except `CARGO_MAKEFLAGS`, `CARGO_BUILD_JOBS`, `CARGO_REGISTRIES_*` and
`CARGO_ENCODED_RUSTFLAGS`. So each `CARGO_*` variable in a job is part of the cache key: two jobs
that differ in one of them, in its name or in its value, share no entry, and each compiles the
crate again.

What slot sets of them, from the table above:

| variable | part of the key | what follows |
|---|---|---|
| `CARGO_MAKEFLAGS`, `CARGO_BUILD_JOBS` | no | none |
| `CARGO_TARGET_DIR`, a tree mirrored at the same path, cargo's own directory | not set | a cargo job, cargo inside a generic job (`slot sh -c 'cargo build'`, `slot task check`) and the jobs of another tree of the repository carry the same `CARGO_*` variables and share entries |
| `CARGO_TARGET_DIR`, a tree mirrored below the workspace root | yes, one value per tree | two trees share no entry; the jobs of one tree do |
| `CARGO_TARGET_DIR`, set by the caller, moved by a cargo configuration, or guessed after a failed `cargo metadata` | yes, the caller's value | jobs with the same value share entries |

slot sets no other `CARGO_*` variable. It sets no `CARGO_NET_OFFLINE`: a cargo job would carry
it and cargo inside a generic job would not, and the two would share nothing. A `CARGO_*`
variable of your own that reaches the job is part of the key like any other: `CARGO_NET_OFFLINE`,
a `CARGO_PROFILE_*` setting, a runner (`CARGO_TARGET_<TRIPLE>_RUNNER`). A job with one shares no
entry with a job without it. Put a setting that every job of a repository should have into the
repository's `.cargo/config.toml` instead of one job's environment.

Not covered by the variables: a crate that includes a file from its build script's output
directory carries that absolute path, and every crate that depends on it is compiled again in
another tree.

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

## Recorded phases of a job

The record of a job in the [job log](../how-it-works.md#job-log) says where its time went. Each
field is absent where it does not apply and in a line an older slot wrote: an absent field means
unknown, never 0.

| field | job | value |
|---|---|---|
| `sync_ms` | remote | milliseconds the client took to send the tree to the node, git directories, a same-path working directory and the declared sibling trees included |
| `sync_bytes` | remote | bytes that sync sent, as rsync counted them |
| `extra_trees` | remote, same-path | the sibling trees the repository's `.slot.toml` declares (`trees`), each synced to its own path on the node before the job ([Sibling trees](../remote-builds.md#sibling-trees)); absent when there were none |
| `back_ms` | remote | milliseconds the client took to copy the cargo products and the `--fetch` paths back after the job ended; absent when nothing came back |
| `compile_ms` | cargo `test`, `nextest`, `bench` | milliseconds from the start to the first test binary, as the job's output shows it: cargo's `Running …` or `Doc-tests …` line, nextest's `Starting N tests across …` line or cargo's JSON `build-finished` message; the whole run when no test binary ran |
| `test_ms` | cargo `test`, `nextest`, `bench` | milliseconds from the first test binary to the end; absent when no test binary ran |
| `node_target_dir` | remote cargo | the target directory the job built into on the node |
| `seeded_from` | remote cargo | what that directory held when the job queued: the workspace id of the sibling it was seeded from by a copy (reflink, or a byte copy with `slot serve --seed-copy copy`), `kept` (the build of an earlier job of the same tree) or `empty` |

A node's record gets `sync_ms`, `sync_bytes` and `extra_trees` with the job's request. `back_ms` is measured
after the job ended: the client sends it to the node in a second request, and the node appends
it to its job log as a line of its own, `{"amend":<id>,"back_ms":<ms>}`, which a reader of job
lines skips and the daemon applies to job `<id>` when it reads the log back. For a job the daemon
hands to a node, its own record gets every remote field from the file in `SLOT_PHASES_FILE`, and
its compile and test time from the output the node streams back.
