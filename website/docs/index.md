---
slug: /
title: slot
description: What slot is, the problem it solves, and what it does not do.
---

# slot

slot runs every build on a machine through one daemon that owns one pool of build slots. The
pool is a GNU-make jobserver: cargo, rustc and GNU make draw their parallelism from it, a go job
holds a fixed share, and any other command holds one slot. A build that has to wait is queued.
The caller sees the command's own output and exit code.

```console
slot cargo test -p foo     # queued on the shared pool; output and exit code as usual
slot go test ./...         # holds 4 slots, runs with -p 4
slot task check            # wrappers pass the pool down to cargo inside
slot --local cargo build   # bypass the daemon
```

## The problem

Each `cargo build` or `go test` assumes it has the whole machine. One build at a time, that is
right. When several coding-agent sessions work on one workstation, each in its own worktree,
their builds overlap, and each one starts as many compiler processes as there are cores. On a
20-core machine, twelve concurrent builds from four sessions have been observed.

Sessions work around this with tools that cannot see each other: `flock` slot files, a hand-set
`CARGO_BUILD_JOBS` per call, or turning off sccache. None of them limits the total. The result is
a long tail: the same command on the same package takes several times longer when other builds
are running. The [design document](https://github.com/beyond10x/slot/blob/main/docs/design.md#1-problem-from-measurement)
has the numbers.

slot replaces those workarounds with one scheduler that owns every CPU slot on the machine.

## What it is

- **One binary.** `slot serve` is the daemon, `slot <command…>` is the client, `slot proxy` is
  the bridge a remote client reaches over ssh.
- **A jobserver pool.** The daemon creates one jobserver FIFO with one token per slot (by default
  one per CPU) and starts every job with it in `MAKEFLAGS` and `CARGO_MAKEFLAGS`.
  [How it works](how-it-works.md) explains the pool, the queue and the web UI.
- **Toolchains.** A command is classified as `rust`, `go` or `other`, which decides how many slots
  it holds and which variables it gets. See [Toolchains](toolchains.md).
- **A remote mode.** With `SLOT_HOST` set, the repository is synced to a build server and the
  build runs there. See [Remote builds](remote-builds.md).
- **Headroom.** `slot headroom 40` keeps 40 % of the CPU out of builds, and call mode raises it
  while a call uses the microphone or camera. See [Headroom](headroom.md).
- **Admission by disk and memory.** A job waits while its filesystem is nearly full or memory
  would run short. See [How it works](how-it-works.md#waiting-for-disk-and-memory).
- **An agent plugin.** A Claude Code hook refuses builds that bypass the pool. See
  [Claude Code plugin](claude-code-plugin.md).

## What it is not

- Not a build cache. sccache caches compiler output; slot schedules the builds that call it.
- Not a build system. Cargo, go, make and task keep doing what they do.
- Not for IDE checks. rust-analyzer runs its own `cargo check`; leave it outside slot.
- Not a multi-user service. One user, one trust domain.
- Linux only. slot reads `/proc` to measure jobs and has no macOS port.

## Next

- [Getting started](getting-started.md): install, run the daemon, run a first build.
- [Headroom](headroom.md): keep CPU free for your desktop and calls.
- [Agent integration](agent-integration.md): the one line to add to a repository's `AGENTS.md`.
- [Claude Code plugin](claude-code-plugin.md): the hook and the warden for agent sessions.
- [CLI reference](reference/cli.md): every command and option.
