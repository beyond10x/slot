---
title: Remote builds
description: Run builds on a build server over ssh, with the repository synced by rsync and target directories kept on the server.
---

# Remote builds

With `SLOT_HOST` set, `slot <command…>` runs the build on another machine. The repository is
synced there with rsync, the daemon on that machine queues the job on its own pool, and output and
exit code come back as for a local build. Target directories stay on the server, which takes
both the CPU load and the disk use off the workstation.

```text
workstation                               build server
slot cargo test -p foo
  1. rsync repository ──ssh──►            <ws-root>/<workspace-id>/
  2. ssh <host> slot proxy ──►            slot serve: queue, pool
                                            cargo test -p foo
                                            CARGO_TARGET_DIR=<cache-root>/target/<workspace-id>
  3. stdout, stderr, exit code ◄──
```

## Set up the server

1. Install slot on the server ([Getting started](getting-started.md#install)) and run
   `slot serve`, for example with the systemd user unit.
2. Make sure a non-interactive ssh session finds `slot` on `PATH`, or set `SLOT_REMOTE` on the
   workstation to its full path on the server. A stock Debian account does not: `~/.local/bin`
   is on `PATH` only in login shells, and `ssh host cmd` is not one.
3. Install `rsync` on both machines.
4. Give the server what the builds need: the toolchains (rustup, go), and read access to any
   private git dependencies or Go modules.
5. Check the disk floor. A job waits while its filesystem has less than `--min-free-disk`
   (default 25 GB) free, and the client prints why. A server with a smaller disk needs a lower
   value, for example `slot serve --min-free-disk 5`.

Jobs on the server start from the daemon's own environment (its `PATH`, `HOME` and toolchain
installs), not from the workstation's. With the systemd unit, that is the unit's `PATH`.

## Use it from the workstation

```console
export SLOT_HOST=buildbox
slot cargo test -p foo
```

`buildbox` is any name `ssh` accepts, including a `Host` entry in `~/.ssh/config`. Key-based
login without a prompt is required.

Remote mode needs a git checkout: the repository root is `git rev-parse --show-toplevel` of the
current directory.

## What happens

1. **Workspace id.** The repository root and the workstation's host name are hashed into a
   16-character id. Each worktree has its own root and therefore its own id, so parallel worktrees
   never share a mirror on the server.
2. **Sync.** rsync copies the repository to `<host>:<SLOT_WSROOT>/<id>/` with `--delete`, a
   filter that applies every `.gitignore`, and excludes for `.git` and the toolchain's output
   directories (`/target/`). Uncommitted changes are included. Over a persistent ssh connection
   a rerun only sends the changed files.
3. **Run.** `slot` starts `ssh <host> slot proxy`, which connects to the server daemon's socket.
   The daemon runs the command in the mirror, in the same subdirectory you called it from.
4. **Caches.** rust and other jobs get `CARGO_TARGET_DIR=<cache-root>/target/<id>`; go and other
   jobs get `GOCACHE=<cache-root>/go-build`. The cache root is `slot serve --cache-root`, by
   default `<state-dir>/cache` on the server.
5. **Output.** Paths under the server's mirror in the output are rewritten to the local
   repository path, so `file:line` in compiler errors points at local files.

`SLOT_WSROOT` on the workstation must name the directory the server daemon uses as `--ws-root`.
The defaults agree: `.local/state/slot/ws` relative to the remote home directory, and
`<state-dir>/ws` with the default state directory `~/.local/state/slot`. If you change one,
change the other.

## Environment

Only variables the job's toolchain admits cross to the server: for rust `CARGO_*` (not
`CARGO_TARGET_DIR`, `CARGO_HOME`, `CARGO_BUILD_JOBS`), `RUST*`, `NEXTEST_*`, `INSTA_*`; for go
`GO*` (not the cache and path variables) and `CGO_ENABLED`; for all, terminal and locale variables.
The full lists are on [Toolchains](toolchains.md). `HOME`, `AWS_*` and other variables no
toolchain admits never cross. A name that contains `TOKEN`, `SECRET`, `PASSWORD`, `PASSWD`,
`CREDENTIAL`, `_KEY` or `AUTH` never crosses through a toolchain's prefix either, so
`CARGO_REGISTRY_TOKEN` and `CARGO_REGISTRIES_<name>_TOKEN` stay on the workstation (slot 0.6.0
and older sent them).

To forward more variables, list their names in `SLOT_ENV`, comma-separated:

```console
export SLOT_ENV=DATABASE_URL,FEATURE_FLAGS
```

## ssh options

By default `slot` calls ssh with a shared control connection, so the sync and the run reuse one
connection, and it lives for ten minutes after the last call:

```text
ssh -o ControlMaster=auto -o ControlPersist=10m -o ControlPath=$XDG_RUNTIME_DIR/slot-ssh-%C -o ConnectTimeout=3
```

`SLOT_SSH` replaces this whole command, split on whitespace, for both rsync and the run:

```console
export SLOT_SSH="ssh -p 2222 -o ConnectTimeout=3"
```

## When the server is unreachable

If the sync fails (the host is down, ssh is refused), `slot` runs the command locally and prints
`slot: daemon unreachable (…), ran locally` on stderr. If the connection drops while the job runs,
`slot` prints `slot: connection to daemon lost` and exits with 3; the daemon cancels the job.

## Tests that need local services

Tests that talk to docker, a database, a local cluster or a port on localhost fail on the server.
List them in `.slot.toml` at the repository root:

```toml
local = ["cargo test -p integration", "go test ./e2e/..."]
```

A command whose first words are one of the entries runs on this machine's daemon, through the
local pool, and slot prints `slot: .slot.toml lists `cargo test -p integration` as local: running
on this machine`. For a one-off, `slot --local <command>` runs the command here without the
daemon.

## Security

Build scripts and tests run arbitrary code on the server. slot assumes the workstation and the
server are one trust domain: one user, a private network (LAN or a VPN overlay), not exposed
publicly. Remote mode uses plain ssh to `slot proxy` and rsync; it does not restrict what the ssh
key can run.

## Not built yet

- **Fetching artifacts back.** Build outputs stay on the server. `cargo build` of a binary you
  then want to run locally needs `--local`, or a copy by hand.
- **Target directory seeding and eviction.** A new worktree starts with an empty target
  directory on the server, and old target directories are never deleted automatically. Watch the
  disk under the cache root.
- **A restricted ssh command.** An `authorized_keys` forced command that accepts only the sync and
  run steps.

## Measured

Against a 4-vCPU KVM guest on the workstation (numbers and method in the repository's
`docs/design.md` § 16), for a workspace of 66 crates and 132 MB of source:

- first sync 1.75 s; a sync with nothing or one file changed 0.29 s;
- cold `cargo build -p ess-cli` 160.7 s; no-op rebuild 0.61 s; one changed file 4.5 s;
- output arrives line by line; `slot cancel`, SIGTERM to the client and a dropped ssh connection
  each end the job on the server.
