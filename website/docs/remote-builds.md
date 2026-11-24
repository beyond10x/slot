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
   a rerun only sends the changed files. The client prints one line per sync, for example
   `slot: synced to buildbox in 0.4 s, 81234 bytes sent, 1 sibling mirror as base`. Two calls
   from one worktree sync one after the other; the second waits for the first.
3. **Siblings.** Before the sync the client asks the server for other mirrors of the same
   repository (same `origin` URL, so every worktree and clone of it). rsync gets up to four of
   them as `--copy-dest`: a file the new mirror lacks is copied on the server when a sibling has
   it unchanged, and a changed file is sent as a delta against the sibling's. The first sync of a
   new worktree then sends what differs from its siblings, not the whole tree. A server whose
   slot does not know the step gets the plain sync; the line then says the bytes were not
   counted.
4. **Run.** `slot` starts `ssh <host> slot proxy`, which connects to the server daemon's socket.
   The daemon runs the command in the mirror, in the same subdirectory you called it from.
5. **Caches.** rust and other jobs get `CARGO_TARGET_DIR=<cache-root>/target/<id>`; go and other
   jobs get `GOCACHE=<cache-root>/go-build`. The cache root is `slot serve --cache-root`, by
   default `<state-dir>/cache` on the server.
6. **Output.** Paths under the server's mirror in the output are rewritten to the local
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

A slow link gets the same treatment. A sync whose rsync reads and writes no byte for
`SLOT_SYNC_STALL` (default 60 s), or that runs longer than `SLOT_SYNC_LIMIT` (default 5 min), is
killed with its ssh, and the job goes to this machine's daemon with `(sync killed: no byte moved
for 60 s)` or `(sync killed: ran over 300 s)` in the line above. An rsync that someone else
kills ends the same way. The job is never sent to the server after a sync that did not finish.

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

## Binaries and test binaries come back

A remote `cargo build`, `cargo test`, `cargo bench` or `cargo rustc` copies what it built for
the workspace's own packages into the local target directory, at the same relative paths, with
modes and modification times:

```console
slot cargo build -p mytool
./target/debug/mytool --version
slot cargo test --no-run
./target/debug/deps/mytest-1f2e3d4c5b6a7980
```

- **What.** Every executable cargo reports for a package of the workspace (binaries, unit and
  integration test binaries, benches, examples), and the files of `cdylib` and `staticlib`
  targets. Artifacts of dependencies, `.rlib` and `.rmeta` files and the rest of the target
  directory stay on the server.
- **When.** After a build that exits 0; after a test or bench run whatever its exit, so the
  binaries of a failed run can be run again here. One line follows, for example
  `slot: fetched 3 artifacts, 41.2 MB, in 0.9 s`. A copy that fails prints
  `slot: artifacts not fetched: …` and turns exit 0 into 1.
- **How.** slot adds `--message-format=json-render-diagnostics` to the command on the server and
  reads cargo's `compiler-artifact` messages from its standard output. It takes those lines out
  again, so you see what cargo would have shown: its diagnostics on stderr, a test's output on
  stdout. If you pass `--message-format json…` yourself, your stream reaches you unchanged and
  slot reads the same messages from it. With `--message-format short` or `human` nothing comes
  back. One `rsync` copies every path.
- **Where to.** `CARGO_TARGET_DIR` of your environment, else `target/` beside the nearest
  `Cargo.lock` above the directory you called from. A `build.target-dir` in a cargo config file
  and `--target-dir` are not read.
- **Freshness.** A file that already matches in size and time is not sent again; a file cargo
  reports as fresh but that is missing here is sent.

## Crates are fetched once per Cargo.lock

Parallel cargo jobs on one server wait in turn on cargo's package-cache lock while one of them
downloads crates. Before a cargo job that builds (`build`, `check`, `clippy`, `test`, `bench`,
`doc`, `rustc`, `nextest`, `llvm-cov`), the server daemon compares the digest of the mirror's
`Cargo.lock` with the one it recorded for that mirror. When they differ it runs
`cargo fetch --locked` once, under a lock per repository, records the digest, and prints
`slot: new Cargo.lock on this mirror: cargo fetch --locked took 2.1 s; its cargo jobs run
offline`. Every such job of the mirror then runs with `CARGO_NET_OFFLINE=true`.

A fetch that fails (a `Cargo.lock` that does not match `Cargo.toml`, an unreachable registry) or
takes longer than 5 minutes sends the job back to this machine's daemon with
`slot: remote build on <host> did not start (cargo fetch --locked failed …); running it on this
machine's daemon`. A mirror without a `Cargo.lock` gets no fetch and runs online. Commands that
need the network themselves (`cargo update`, `add`, `install`, `fetch`) run online.

The digests live in `<ws-root>/.cargo-fetch/` on the server and go with their mirror when it is
evicted. Both features need this release of slot on the server and on the workstation.

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

## Mirror at the same path

Tests and scripts that name an absolute path into the checkout (`/srv/u/src/app/scripts/check`)
fail in `<ws-root>/<id>`, and the compile cache sees another working directory. When the server's
account has the same home directory as the workstation's, the tree can be mirrored at its own
absolute path instead:

```console
export SLOT_REMOTE_SAME_PATH=1
```

The sync then goes to the checkout's absolute path on the server and the job runs in the same
directory as here. The server admits such a tree only below a directory it names, given to both
`slot serve` and `slot shell` (repeat the option, or list them in `SLOT_SAME_PATH_ROOTS`
separated by `:`):

```text
slot serve --same-path-root /srv/u
command="/usr/local/bin/slot shell --same-path-root /srv/u",restrict ssh-ed25519 AAAA… workstation
```

A tree outside those roots is refused: `slot shell` refuses the sync, so the job runs on this
machine's daemon, and a daemon without the root ends the job with exit 2 and `not below a
--same-path-root`. `<ws-root>/<id>` becomes a link to the tree, so the next worktree of the same
repository is still seeded from it (`--copy-dest=<ws-root>/<id>`), and the target directory stays
`<cache-root>/target/<id>`. With `--sandbox`, the tree is bound writable at its own path.
Eviction deletes the link and the target directory, never the tree. Needs this release of slot
on both machines.

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

## Show the server in the web view

The workstation's daemon shows a build server as a node of its web view when the host
configuration file names it, `~/.config/slot/cluster.toml` (`$XDG_CONFIG_HOME/slot/cluster.toml`;
`slot serve --cluster-file` or `SLOT_CLUSTER_FILE` names another, `off` none):

```toml
[remote.build-1]
ssh = "ssh -o BatchMode=yes build-1.example"
slot_path = "/usr/local/bin/slot"
deadline = "2030-01-01T18:00:00Z"
price_per_hour_cents = 250
location = "region-a"
machine_type = "64-vcpu"
```

Only `ssh` is required. The daemon reads the file at start. Every 5 s it runs `<ssh> <slot_path>
state` for each node: the server's `slot shell` answers with its daemon's own row of
`/api/nodes` and its queued, running and last 30 finished jobs. `GET /api/nodes` then lists the
node (kind `Remote`, its CPUs, slots, memory, disk and load as the server reports them, the
deadline and price from the file), and `GET /api/state` and the job events carry its jobs with
`node` set to the node's name and an id below 0, so they never meet a local id. A node is
`Joining` until its first answer; a poll that fails or takes longer than 4 s is missed, and after
three misses in a row the node is `Gone` (reason `Lost`) with its last row; the next answer makes
it `Ready` again. A key of the file that the shape does not declare is refused, and the daemon
then polls no node.

## Hand queued jobs to a node

A node of the host configuration file also takes jobs from the workstation's queue: while it is
`Ready`, before its deadline and has free slots, the daemon hands it queued local jobs, oldest
first, without a `SLOT_HOST` in the caller's environment. The job's process is then
`slot run -- <command>` in remote mode, started by the daemon with the job's directory and
environment and `SLOT_HOST`, `SLOT_SSH` and `SLOT_REMOTE` from the node's `ssh` line (its last
word is the host, the words before it the ssh command) and `slot_path`, and
`SLOT_REMOTE_SAME_PATH=1`: the node runs it at the same path as here. It holds no local slot;
its output and exit code reach the caller as before, `slot status` prints `spill: N jobs to
<node>`, and the job's record in `GET /api/state` has `node` set to the node. The node's own
row of the same job appears beside it, with an id below 0.

```toml
spill_after_s = 0

[remote.build-1]
ssh = "ssh -o BatchMode=yes build-1.example"
spill = true
```

- `spill` (default `true`): `false` keeps the node out of it.
- `spill_after_s` (default 0): seconds a job waits in the local queue first.

A node takes at most its free slots from its last answer, less the jobs handed to it in the last
15 s, and none while it has a queue of its own. Every job goes but a job nested in another, one
submitted with `slot --local` (which never reaches the daemon) or with `SLOT_NO_REMOTE=1` in its
environment, a remote client's job and a gate's hold. When the client ends with exit 3 (the sync
or ssh failed, or the connection to the node was lost) the job goes back to the local queue once
and runs here. A command that itself exits 3 is run a second time here too.

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
--sender` of one or more paths, each in the workspace root or the cache root's `target/`),
`slot proxy`, `slot cancel <id>`, the sibling step (`slot __siblings` on the server's own
workspace root) and the read-only `state` step (`slot state`: the server daemon's own row of
`/api/nodes` and its jobs, read from `--http`, for the
[node poll](#show-the-server-in-the-web-view)). Anything else
exits 126 with `slot shell: refused: …`. rsync options that name another file or program (`-e`,
`--rsync-path`, `--log-file`, `--files-from`, `-T`, `--write-batch`) are refused, and
`--copy-dest` is taken only as `../<id>`, a sibling mirror in the workspace root. Also refused:
`..` in paths, shell operators and quotes that would expand. `restrict` turns off
port, agent and X11 forwarding and the terminal. Pass `--ws-root`, `--cache-root` or
`--state-dir` to `slot shell` when `slot serve` uses other values.

This narrows the key to slot's protocol. It is not a sandbox: a job is any command, so whoever
holds the key can still run code on the server as that user through `slot proxy`. What that code
can write is what `--sandbox` below confines.

### Run every job in a sandbox

`slot serve --sandbox` runs each job under bubblewrap (`bwrap`), so one build cannot delete,
replace or install a program another build needs, or change another build's tree:

- The whole filesystem is read-only to the job: the home directory, `~/.rustup`, `~/.cargo/bin`,
  `/usr`, `/usr/local`.
- Writable: the job's workspace (or, for a local job, the git checkout it runs in), its target
  directory, a private `/tmp` (`TMPDIR=/tmp`), cargo's `registry/` and `git/` with their lock
  files, the Go build and module caches, and each path given with `--sandbox-write <path>`.
- Hidden: the other workspaces, the other jobs' target directories, the user's runtime directory
  with the user bus, and the daemon's socket.
- The job has its own pid namespace: it sees and signals only its own processes.
- The network is the host's: the compile cache server answers on TCP and cargo may fetch.
- `SLOT_SANDBOX=bwrap` is set in the job's environment.

Install bubblewrap first (`dnf install -y bubblewrap` on Amazon Linux and Fedora,
`apt install bubblewrap`, `pacman -S bubblewrap`). Without a `bwrap` that can create the
sandbox, `slot serve --sandbox` does not start; it never runs a job outside it.

```sh
slot serve --sandbox --sandbox-write /srv/pnpm-store
```

A build that writes somewhere else (a package manager's store, a browser download) fails with
`Read-only file system` until that path is named with `--sandbox-write`. Every path and its
reason: the repository's `docs/design.md` § 27.

With `--sandbox-fixed-paths` as well, a job sees its checkout at `/work/src` and its target
directory at `/work/target` (`CARGO_TARGET_DIR=/work/target`), so every worktree of a repository
builds at the same paths and `CARGO_MANIFEST_DIR` is the same in each. Off by default: the job
then runs without a rustc wrapper, because the compile cache server outside the sandbox cannot
read sources at those paths.

## Measured

Against a 4-vCPU KVM guest on the workstation (numbers and method in the repository's
`docs/design.md` § 16), for a workspace of 66 crates and 132 MB of source:

- first sync 1.75 s; a sync with nothing or one file changed 0.29 s;
- cold `cargo build -p ess-cli` 160.7 s; no-op rebuild 0.61 s; one changed file 4.5 s;
- output arrives line by line; `slot cancel`, SIGTERM to the client and a dropped ssh connection
  each end the job on the server.
