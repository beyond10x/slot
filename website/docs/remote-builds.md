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

A cloud machine with NVMe instance storage (Amazon Linux 2023) can be set up by the scripts in
[`contrib/node/`](https://github.com/beyond10x/slot/tree/main/contrib/node): `provision.sh` on
the machine and `push.sh` on the workstation, both with a `check` that lists where the machine
differs. Names, keys and paths come from a config file outside the repository.

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
   directories (`/target/`). Uncommitted changes are included. rsync compares every file by its
   content (`--checksum`), not by size and modification time, so a file changed at the same size
   and time is sent too; this reads every file on both sides on each sync. The mirror does not
   take the workstation's modification times (`--no-times`): a file whose content changed gets
   the server's time of the sync, and a file whose content did not change keeps the time it has
   on the server. cargo decides by modification time, so a change that was made on the
   workstation before a build the server ran on the old content, and synced after it, is still
   newer than that build and is compiled; and a `touch` on the server outlives the next sync.
   Over a persistent ssh
   connection a rerun only sends the changed files. The client prints one line per sync, for
   example
   `slot: synced to buildbox in 0.4 s, 81234 bytes sent, at 1a2b3c4 + 3 changed files`:
   the commit checked out and the number of files `git status --porcelain` lists. Two calls
   from one worktree sync one after the other; the second waits for the first.
   Symlinks stay links. A link whose target lies outside the tree is sent as the link, never as
   the file it points to, so it dangles on the server unless the target exists there too; the
   line counts such links: `2 symlinks leave the tree (sent as links)`. Before the sync, the
   client lists the tree with the same `rsync` it syncs with (`--list-only`, nothing is sent).
   If that `rsync` would send the target in place of such a link (a wrapper on `PATH` that adds
   `--copy-links` or `--copy-unsafe-links`), the sync is refused and the job runs on this
   machine's daemon (see [When the server is unreachable](#when-the-server-is-unreachable)).
3. **Siblings.** Before the sync the client asks the server for other mirrors of the same
   repository (same `origin` URL, so every worktree and clone of it). rsync gets up to four of
   them as `--copy-dest`: a file the new mirror lacks is copied on the server when a sibling has
   the same content, and a changed file is sent as a delta against the sibling's. The first sync
   of a new worktree then sends what differs from its siblings, not the whole tree; the line then
   names `1 sibling mirror as base`. A server whose
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
toolchain admits never cross.

To forward more variables, list their names in `SLOT_ENV`, comma-separated:

```console
export SLOT_ENV=DATABASE_URL,FEATURE_FLAGS
```

A name that looks like a credential never crosses, whether a toolchain admits it or `SLOT_ENV`
lists it: one that contains `TOKEN`, `SECRET`, `PASSWORD`, `PASSWD`, `PASS`, `CREDENTIAL`,
`_KEY`, `AUTH`, `SSH_` or `AWS_SECRET`, or that has `PAT`, `DSN` or `KEY` as a word between
underscores (`NPM_PAT`, `SENTRY_DSN`; not `PKG_CONFIG_PATH`). So `CARGO_REGISTRY_TOKEN` and
`DB_PASS` stay on the workstation. The client names each one it held back on stderr, never the
value:

```text
slot: not sent to buildbox, the names look like a credential: DB_PASS, NPM_PAT
```

slot 0.6.0 and older sent such names through a toolchain's prefix; releases before this one sent
any name `SLOT_ENV` listed.

## Trees that never leave this machine

A file of path prefixes lists trees whose content is never synced to a server: one absolute path
per line, `#` starts a comment. Its path is `never_sync_file` in the host configuration
(`$XDG_CONFIG_HOME/slot/cluster.toml`), by default `$XDG_CONFIG_HOME/slot/remote-deny`:

```text
# customer data
/srv/work/customer-a
/srv/work/customer-b/
```

A job is never synced when one of these lies under a listed prefix: its working directory, its
checkout's root, its checkout's git directories (a linked worktree's are in its main
repository), or a tree an argument names (`--manifest-path`, `-C`, `--dir`, `--directory`). The
same holds for a tree that is `/`, the home directory or a directory above it. A prefix matches
whole path components (`/srv/work/a` does not hold `/srv/work/ab`), on the paths as written and
with their symlinks resolved.

Such a job runs in this machine's pool with the reason `never sync: <prefix>` (`never sync: home
directory`), which also keeps the daemon from handing it to a node. With `SLOT_HOST` set the
client says so:

```text
slot: never sync: /srv/work/customer-a: the tree is not synced to buildbox; running on this machine
```

The check is in the `slot` binary, so it holds however the binary is called: with `SLOT_HOST`
already set, through a wrapper or without one. A list file that cannot be read, or a host
configuration that does not parse, keeps every job on this machine.

The same lists hold for the [git dependencies](#git-dependencies-come-from-this-machine) the
checkout's lock files name, together with a second file,
`$XDG_CONFIG_HOME/slot/remote-deny-origins`. Either file may hold either kind of line:

```text
/srv/work/customer-a
origin:*git.example.org[:/]team/*
```

- `/abs/path`: a path prefix, as above. A `git+file` source whose path lies under it is denied.
- `origin:<pattern>`: a shell pattern, matched as a shell's `case` matches it (`*` any run of
  characters, `/` included; `?` one character; `[...]` and `[!...]` classes), against the whole
  URL cargo fetches: the source without `git+`, its `?query` and its `#revision`
  (`git+https://git.example.org/team/dep.git?branch=main#…` is
  `https://git.example.org/team/dep.git`). The pattern above denies that URL and
  `ssh://git@git.example.org/team/dep`, not `https://git.example.org/teamwork/dep.git`.

Other lines are skipped; the tree check above ignores `origin:` lines. A job stays on this machine
when a `git+` source of one of its checkout's `Cargo.lock` files is denied, with the reason
`git dependency denied: <source>`:

```text
slot: git dependency denied: git+https://git.example.org/team/dep.git?branch=main: the tree is not synced to buildbox; running on this machine
```

With both files naming nothing no lock file is read.

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
  otherwise. `slot serve --seed-copy copy` takes a byte copy where there are no reflinks. The
  copy is never a hard link: the two worktrees share no file of their target directories.
  After the copy, every source file of the new worktree's mirror that has the same bytes as the
  other worktree's takes that file's modification time, and every other one a time after every
  file of the copy, so cargo builds each crate whose sources differ from the other worktree's
  and takes the rest as built. The job's stderr says `target directory seeded from workspace
  <id> by a copy in 0.4s; 3 source file(s) differ from its sources and build again`, and its
  record names the workspace in `seeded_from`.
- **Eviction.** Before a remote job queues, the daemon deletes target directories and their
  source copies, least recently used first, while together they take more than `slot serve
  --cache-budget` GB, or while the disk has less than `--min-free-disk` free. A worktree with a
  queued or running job is never deleted. The job's stderr and the journal name every deleted
  workspace. `--cache-budget 0` (the default) leaves only the disk floor.

## Warm the node after a merge

`slot warm [DIR]` tells the daemon that `main` of a repository moved. Run it in the checkout of
`main` after it took the merge, for example from that checkout's `.git/hooks/post-merge`:

```sh
#!/bin/sh
slot warm
```

It returns at once. The daemon builds the tests of that checkout, from its root, as a warm-up
job: `cargo test --no-run`, or the command `.slot.toml` names. On a node that job builds into the
checkout's own target directory, and when it ends with exit 0 that directory is the most
recently used one of the repository: the next new worktree is seeded from it.

```toml
warm = "task warm"
```

`warm = false` switches it off; `slot warm` then sends nothing. The command is split at
whitespace.

- The daemon starts one only for a repository a job built on it in the last 24 hours, and one at
  a time per repository; otherwise it answers `slot: <repo> not warmed: <why>`. `slot warm`
  exits 0 either way.
- The job is queued at `low` whatever `slot priority` says, and never promoted.
- A job of a session that still waits for its slots 50 ms after it queued cancels every running
  warm-up. The warm-up's record ends `cancelled` with `cancel_reason` `preempted by job <id>`.
- A daemon that hands jobs to a node hands a warm-up as `slot run --warm -- <command>`, and the
  node runs it as its own warm-up. A warm-up goes only while no other job waits for a node, and
  never holds one back.
- A queued warm-up is dropped at a daemon reload; a running one is handed over.

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
- **Older copies.** cargo names a test binary `<name>-<hash>`, and a new version, feature set or
  profile gives it a new hash. After the copy, slot removes from the same local directory every
  file named `<name>-<16 hex digits>`, with or without an extension (`.d`, `.dwp`), whose hash
  is not one it just copied, for each `<name>` it just copied. Other files and directories stay.
  The products line then ends with, for example, `; removed 2 superseded files, 38.4 MB`.
- **Off.** `SLOT_NO_PRODUCTS=1` in your environment copies nothing back and runs your command
  unchanged; slot prints `slot: artifacts not fetched: SLOT_NO_PRODUCTS is set` instead.

## Crates are fetched once per Cargo.lock

Parallel cargo jobs on one server wait in turn on cargo's package-cache lock while one of them
downloads crates. Before a cargo job that builds (`build`, `check`, `clippy`, `test`, `bench`,
`doc`, `rustc`, `nextest`, `llvm-cov`), the server daemon compares the digest of the mirror's
`Cargo.lock` with the one it recorded for that mirror. When they differ it runs
`cargo fetch` once, under a lock per repository, records the digest, and prints
`slot: new Cargo.lock on this mirror: cargo fetch took 2.1 s; its cargo jobs run
offline`. Every such job of the mirror then runs with `CARGO_NET_OFFLINE=true`.

Like cargo on your machine, the fetch updates a `Cargo.lock` that is out of date, for example
after you add a path dependency. A job that passes `--locked` or `--frozen` itself gets
`cargo fetch --locked` instead, so it fails on the server as it would locally.

A fetch that fails (a `Cargo.toml` cargo cannot resolve, an unreachable registry, an out-of-date
`Cargo.lock` under `--locked`) or takes longer than 5 minutes sends the job back to this
machine's daemon with
`slot: remote build on <host> did not start (cargo fetch failed …); running it on this
machine's daemon`. A mirror without a `Cargo.lock` gets no fetch and runs online. Commands that
need the network themselves (`cargo update`, `add`, `install`, `fetch`) run online.

The digests live in `<ws-root>/.cargo-fetch/` on the server and go with their mirror when it is
evicted. Both features need this release of slot on the server and on the workstation.

## Git dependencies come from this machine

A `git+` source in a `Cargo.lock` may be reachable only from your machine: a private forge the
server holds no credential for, or no network to. The fetch above runs on the server, and only for
a cargo command slot sends itself; a wrapper's inner cargo call (`task check`, `make`) fetches on
the server too.

So before every remote job the client reads each `Cargo.lock` of the checkout that the sync sends,
and takes the `git+` sources and their locked revisions. A revision missing from this machine's
cargo git database (`$CARGO_HOME/git/db`) gets `cargo fetch --locked` in that lock file's
directory here, with your credentials and outside the pool. The client then asks the server which
of those database entries (`<name>-<hash>`) lack the revision, and copies only those into the
server's `$CARGO_HOME/git/db` (`~/.cargo/git/db` when it sets none) over the same ssh and rsync:

```text
slot: git dependencies: 1 copied to buildbox in 0.4 s
```

cargo on the server then finds the locked revision in its database and does not fetch it. A job
whose entries are all on the server copies nothing and prints nothing. Only git objects cross:
no credential, no `FETCH_HEAD`. A source on a deny list keeps the job here
([above](#trees-that-never-leave-this-machine)).

A step that fails does not stop the job: `slot: git dependency not here: …` names a revision this
machine could not fetch, and `slot: git dependencies not copied to <host>: …` a probe or copy that
failed (a server with an older slot answers no probe); the job then runs on the server as before.
The server needs this release of slot for `slot shell` to admit the probe (`slot __cargo-git`)
and the copy.

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
repository is still seeded from it (`--copy-dest=<ws-root>/<id>`). Eviction deletes the link,
never the tree. Needs this release of slot on both machines.

The job then runs as on a CI runner, with everything at the workstation's paths:

- The tree, with uncommitted and untracked files that git does not ignore.
- The git directory, synced at its own path, so `git` works in the job. For a linked worktree
  that is the repository's common git directory, which holds the worktree's own; it is shared by
  every worktree, so after the first sync only changes move. `*.lock` files stay behind.
- The target directory: `CARGO_TARGET_DIR` when it is set here, else the one `cargo metadata`
  names for the command's manifest (`--manifest-path driver/Cargo.toml` builds into
  `driver/target`). The paths cargo prints, and `CARGO_BIN_EXE_*` in tests, are valid on both
  machines, and a build's products come back to the same paths. It must lie below a
  `--same-path-root` too.
- A working directory the sync leaves out, such as `target/scratch/cli` or another git-ignored
  directory, is synced on its own, with its own `.gitignore` files and without its `target/`.
- A directory outside any git checkout (a scratch copy) is the tree itself.

With `--sandbox`, the tree and the target directory are bound writable at their own paths; the
git directory is read-only. Files the command names outside the tree are not synced: a job that
reads them fails on the server.

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
deadline and price from the file; the machine's boot, its scheduled poweroff, its whole-host CPU and
load, and the jobs it ended in the last 60 minutes and per hour of the last 25, by result, as the
server reports them), and `GET /api/state` and the job events carry its jobs with
`node` set to the node's name, the id the node gave the job, and `from_node: true`: a
node's job is named by its node and its id together, since the id may repeat a local one
(`slot status` prints `#7089 on build-1d`). The output of a node's job is
`GET /api/jobs/<id>/output?node=<name>`: the daemon reads it from the node with `<ssh> <slot_path>
output <id> <from>` every 2 s until the job ended. The deadline and price are read again when the file
changes, at the next poll. A node is
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
row of the same job appears beside it, with the node's id and `from_node: true`.

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

A job that runs or waits here while a node takes jobs carries the reason in `local_reason`
(`/api/state`), and `slot status` prints it under the job's line as `  local: <reason>`:
`local requested` (`SLOT_NO_REMOTE=1`, or a command `.slot.toml` lists as local), `cache rule`
(the working directory or an argument under the caller's `~/.cache`), `nested job`, `gated job`,
`remote job`, `sent back by a node`, `slots free here first` (it got slots here before a node
took it), or `sync failed: <first line of the error>` from a remote-mode client whose start
failed and that ran the job on this machine's daemon instead. A wrapper sets its own reason with
`SLOT_LOCAL_REASON=<text>`; a job with a reason is not handed to a node.
`slot --here <command>` keeps a job that needs this host's credentials while it runs (a clone, a
fetch, a push) in this host's pool, whatever `SLOT_HOST` says, with the reason `here: needs this
host`; unlike `--local` it still waits for slots in the daemon's queue.

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
workspace root), the read-only git dependency probe (`slot __cargo-git <entry>:<rev>…`) and the
copy of one [git dependency](#git-dependencies-come-from-this-machine) (`rsync --server` into
one entry directly under `$CARGO_HOME/git/db`, never read back) and the read-only `state` step (`slot state`: the server daemon's own row of
`/api/nodes` and its jobs, read from `--http`, for the
[node poll](#show-the-server-in-the-web-view)) and the read-only `output` step (`slot output <id>
[<from>]`: the kept output of one job from its chunk `<from>` on, read from `--http`). Anything else
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
