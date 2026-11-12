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
   value, for example `slot serve --min-free-disk 5`. The memory floor works the same way:
   `--min-free-memory` (default 4 GB) plus a job's expected peak must be available, so on an 8 GB
   server a second large build waits for the first; lower it there.

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

If the remote job cannot start (the host is down, ssh is refused, the sync fails, for example
because the server disk is full), `slot` sends the job to this machine's daemon instead and prints
`slot: remote build on <host> did not start (…); running it on this machine's daemon`. The job
then waits for slots in the local pool like any other. Before 0.8.0 it ran the command directly,
outside the pool. If the connection drops while the job runs, `slot` prints `slot: connection to
daemon lost` and exits with 3; the daemon cancels the job.

## Target directories on the server

Each worktree has its own target directory on the server, `<cache-root>/target/<workspace-id>`,
next to a record `<workspace-id>.json` of its repository, last use and size.

- **Seeding.** The first job of a new worktree starts from a copy of the most recently used
  target directory of another worktree of the same repository, so dependencies are not compiled
  again. The copy is a reflink (`cp --reflink=always`): instant, and sharing blocks until cargo
  rewrites them. Where the filesystem has no reflinks (ext4), nothing is copied: a plain copy of
  a 4 GB target directory took longer than the compile time it saved. Put the cache root on btrfs
  or xfs to get seeding; the journal says `target directory not seeded: no reflink copy …`
  otherwise.
- **Eviction.** Before a remote job queues, the daemon deletes target directories and their
  source copies, least recently used first, while together they take more than `slot serve
  --cache-budget` GB, or while the disk has less than `--min-free-disk` free. A worktree with a
  queued or running job is never deleted. The job's stderr and the journal name every deleted
  workspace. `--cache-budget 0` (the default) leaves only the disk floor.

## Fetch build output back

Build output stays on the server. `--fetch PATH` copies a path back into the checkout after the
job exits 0:

```console
slot --fetch target/debug/mytool cargo build -p mytool
./target/debug/mytool --version
```

`PATH` is relative to the repository root. A path under `target/` is read from the job's target
directory on the server (the server sets `CARGO_TARGET_DIR` per workspace); any other path from
the server's copy of the repository, for output a build writes into the source tree. Repeat
`--fetch` for more paths. A failed copy prints `slot: fetch …` and makes the exit code 1. With no
`SLOT_HOST`, or for a command `.slot.toml` keeps local, the output is already here and `--fetch`
does nothing. Fetching from `target/` needs slot 0.8.0 or later on the server, which reports the
target directory when the job starts.

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
publicly.

### Restrict the key to slot

`slot shell` is a forced command for the server's `~/.ssh/authorized_keys`:

```text
command="/usr/local/bin/slot shell",restrict ssh-ed25519 AAAA… workstation
```

ssh then runs `slot shell` for every connection with that key, and it runs only the steps of
remote mode: the sync (`rsync --server` into the workspace root), a fetch (`rsync --server
--sender` from the workspace root or the cache root's `target/`), `slot proxy` and `slot cancel
<id>`. Anything else exits 126 with `slot shell: refused: …`. rsync options that name another
file or program (`-e`, `--rsync-path`, `--log-file`, `--files-from`, `-T`, `--write-batch`) are
refused, as are `..` in paths, shell operators and quotes that would expand. `restrict` turns off
port, agent and X11 forwarding and the terminal. Pass `--ws-root`, `--cache-root` or
`--state-dir` to `slot shell` when `slot serve` uses other values.

This narrows the key to slot's protocol. It is not a sandbox: a job is any command, so whoever
holds the key can still run code on the server as that user through `slot proxy`.

## Measured

Against a 4-vCPU KVM guest on the workstation (numbers and method in the repository's
`docs/design.md` § 16), for a workspace of 66 crates and 132 MB of source:

- first sync 1.75 s; a sync with nothing or one file changed 0.29 s;
- cold `cargo build -p ess-cli` 160.7 s; no-op rebuild 0.61 s; one changed file 4.5 s;
- output arrives line by line; `slot cancel`, SIGTERM to the client and a dropped ssh connection
  each end the job on the server.
