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
| `SLOT_SOCKET` | `$XDG_RUNTIME_DIR/slot.sock` | the local daemon's socket |
| `SLOT_TOKENS` | unset | slots a job holds, as `slot --tokens N` (which wins over it); capped at the slots in circulation, ignored with `--local`. See [Toolchains](../toolchains.md#naming-the-slots). |
| `SLOT_HTTP` | `127.0.0.1:7411` | the daemon's web address for `slot status`, `slot headroom` and `slot sessions` |
| `SLOT_HOOK` | `block` | mode of `slot hook claude-pre-tool-use`: `block` refuses a build outside the pool, `warn` only adds advice, `off` allows everything. See [Claude Code plugin](../claude-code-plugin.md). |

## Read by the daemon

| variable | effect |
|---|---|
| `SLOT_SOCKET` | default for `slot serve --socket` and `slot proxy --socket` |
| `XDG_RUNTIME_DIR` | directory of the default socket; without it, `slot-<uid>.sock` in the temporary directory |
| `XDG_STATE_HOME` | the default state directory is `$XDG_STATE_HOME/slot`, otherwise `~/.local/state/slot` |

## Set in every job

| variable | value |
|---|---|
| `SLOT_JOB` | the job id, used to attribute sccache CPU time to the job |
| `MAKEFLAGS`, `CARGO_MAKEFLAGS` | `-j --jobserver-auth=fifo:<state-dir>/jobserver.fifo`, for rust and other jobs |
| `RUST_TEST_THREADS`, `NEXTEST_TEST_THREADS` | `slot serve --test-threads` (default a quarter of the pool, at least 2); rust and other jobs, unless the caller set them |
| `GOMAXPROCS` | slots held for go jobs; `--test-threads` for other jobs; unless the caller set it |
| `GOFLAGS` | the caller's value plus `-p=<slots held>`; go jobs |
| `CARGO_TARGET_DIR` | `<cache-root>/target/<workspace-id>`; remote rust and other jobs |
| `GOCACHE` | `<cache-root>/go-build`; remote go and other jobs |

## Dropped from the caller's environment

`MAKEFLAGS`, `MFLAGS`, `CARGO_MAKEFLAGS` and `CARGO_BUILD_JOBS`. Each would let a job bypass the
pool.

A local job otherwise gets the caller's full environment. A remote job starts from the server
daemon's environment, plus the variables its [toolchain](../toolchains.md) admits and the names
in `SLOT_ENV`. A name that looks like a credential (it contains `TOKEN`, `SECRET`, `PASSWORD`,
`PASSWD`, `CREDENTIAL`, `_KEY` or `AUTH`) crosses only when `SLOT_ENV` names it.
