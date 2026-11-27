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
the machine (a bare image becomes a complete node; nothing is copied from the workstation) and
`push.sh check` on the workstation, which lists where the node's rust, go and node versions
differ from the workstation's (the node installs go and node at pinned releases). That check is a
version parity report, not an integrity check: it runs as the build user, and a job that runs as
that user can alter what the node reports. Every download `provision.sh` makes is checked against
a pinned SHA-256 before anything is unpacked or run, and `provision.sh check` names a running unit
whose definition or slot binary is not the one on disk. Names, keys and paths come from a config
file outside the repository.

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
   The daemon runs the command in the mirror, in the same subdirectory you called it from. While
   that daemon does not answer, the job waits and is sent again (see [When the server's daemon
   does not answer](#when-the-servers-daemon-does-not-answer)).
5. **Caches.** rust and other jobs get `CARGO_TARGET_DIR=<cache-root>/target/<id>`; go and other
   jobs get `GOCACHE=<cache-root>/go-build`. The cache root is `slot serve --cache-root`, by
   default `<state-dir>/cache` on the server.
6. **Output.** Paths under the server's mirror in the output are rewritten to the local
   repository path, so `file:line` in compiler errors points at local files.
7. **Return.** What the job wrote in its tree comes back, and what it deleted there is deleted
   here (see [What the job wrote or deleted comes back](#what-the-job-wrote-or-deleted-comes-back)).

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
repository), a tree an argument names (`--manifest-path`, `-C`, `--dir`, `--directory`), or a
tree `.slot.toml` declares ([Sibling trees](#sibling-trees)). The
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

Nothing failed, so the job runs here without `SLOT_REMOTE_FALLBACK`, and it runs here whatever
else the caller's environment holds. One call ends with exit 3 instead, and its line has no
`; running on this machine`: the client that this machine's daemon started itself for a job it
handed to a node ([Hand queued jobs to a node](#hand-queued-jobs-to-a-node)). The daemon gives
that client `SLOT_SPILLED` and `SLOT_PHASES_FILE`, and it queues the job here again when the
client ends that way. `SLOT_SPILLED` alone, set by a wrapper beside `SLOT_HOST`, is no such
client: up to 0.39.1 its job ended with exit 3 and ran nowhere; now it runs in this machine's
pool.

The same lists hold for the [git dependencies](#git-dependencies-come-from-this-machine) the
checkout's lock files name, together with a second file,
`$XDG_CONFIG_HOME/slot/remote-deny-origins`. Either file may hold either kind of line:

```text
/srv/work/customer-a
origin:*git.example.org[:/]team/*
```

- `/abs/path`: a path prefix, as above. A `git+file` source whose path lies under it is denied.
- `origin:<pattern>`: a shell pattern, matched as a shell's `case` matches it (`*` any run of
  characters, `/` included; `?` one character; `[...]` and `[!...]` classes), without regard to
  case, against the whole URL cargo fetches: the source without `git+`, its `?query` and its
  `#revision` (`git+https://git.example.org/team/dep.git?branch=main#…` is
  `https://git.example.org/team/dep.git`). The pattern above denies that URL and
  `ssh://git@git.example.org/team/dep`, not `https://git.example.org/teamwork/dep.git`.

`origin` may be written in any case, with spaces around the `:`, and ` #` starts a comment. Any
other line that is neither empty nor a comment keeps every job on this machine, with a reason
naming the file and the line; the tree check above ignores `origin:` lines.

Each URL is matched in several forms, and one denied form is enough: as written; normalised
(scheme and host in lower case, without a trailing dot of the host, a default port or a doubled
`/`, a `file` path percent-decoded, `user@host:path` as `ssh://user@host/path` and
`[user@host:port]:path` as `ssh://user@host:port/path`); bare, without user and port and without
a trailing `/` or `.git`, as `scheme://host/path` for its own scheme and for `https`, `http`,
`ssh`, `git`, `git+ssh` and `ssh+git`, and as `host/path`; the URL git fetches after a
`url.<base>.insteadOf` rule; and for ssh the host name `ssh -G` resolves an alias to. So a URL
is denied when the pattern matches its full form or its host and path:
`*git.example.org[:/]team/*` also denies `ssh://git@git.example.org:2222/team/dep`,
`https://git.example.org/team/*` also denies `https://user@git.example.org/team/dep` and
`ssh://git@git.example.org/team/dep`, and a pattern ending in `dep.git` also denies the URL
ending in `dep`. Write patterns without a user or a port: a pattern that names them matches only
URLs that name them.

Matching does not resolve names: an IP address or another DNS name for a denied host is not
caught by an `origin:` pattern. Nor is an HTTP redirect: git follows one from an allowed host to
a denied one, and the check sees only the URL before it.

A URL that cannot be taken apart is denied, and so is a lock file that does not parse or a `git+`
source without a full revision while either list names anything. For a checkout with a
`Cargo.lock`, the job also stays here when `git` or `ssh` cannot answer the lookup within 10 s,
when the cargo home is not an absolute path (`CARGO_HOME` relative, or neither `CARGO_HOME` nor
`HOME` set), or when a cargo configuration replaces a git source (a `[source.<name>]` table with
`replace-with` where it or its replacement has `git`, in a `.cargo/config.toml` of the checkout,
above it or in the cargo home): cargo would then build another repository than the URL the lock
file names. A cargo configuration with a top-level `include` keeps the job here too: the files
it names are not read. A job stays on this machine when a `git+` source of one of its checkout's
`Cargo.lock` files is denied, with the reason `git dependency denied: <source>`:

```text
slot: git dependency denied: git+https://git.example.org/team/dep.git?branch=main: the tree is not synced to buildbox; running on this machine
```

The job then runs in this machine's pool as the job of a listed tree does: neither the tree nor
the dependency is sent, its exit code is the command's, and its record carries the reason in
`local_reason`. Exit 3 is kept for the one call named above, the client of a job this machine's
daemon handed to a node.

With both files naming nothing no lock file is read.

### Only trees with an allowed remote

The deny lists name what someone thought of in advance: a repository without a remote matches no
`origin:` pattern. An allow list turns that around. When `$XDG_CONFIG_HOME/slot/remote-allow-origins`
exists (`allow_origins_file` in the host configuration names another absolute path), a tree goes
to a node only when one of its own remotes (`remote.<name>.url` of the repository's own
configuration) matches a line, and no `origin:` pattern of the deny lists matches one of its
remotes:

```text
# the forges whose repositories may be built on a node
*git.example.org[:/]team/*
origin:https://forge.example/org/*
```

One shell pattern per line, matched as an `origin:` pattern is (an `origin:` before it is
allowed, ` #` starts a comment), against each remote URL as written, normalised and bare, without
a lookup: `url.<base>.insteadOf` and ssh aliases are not resolved. A repository with no remote, a
folder without git and a repository whose remotes the list does not name run in this machine's
pool with the reason `never sync: no allowed remote (<tree>)`:

```text
slot: never sync: no allowed remote (/srv/work/private): the tree is not synced to buildbox; running on this machine
```

The client checks the job's own tree, with or without `SLOT_HOST`, the tree of a job the daemon
hands to a node, and every tree `.slot.toml` declares ([Sibling trees](#sibling-trees)); a declared
tree that is refused keeps the whole job here (`never sync: no allowed remote (declared tree
/srv/u/specs)`). An empty file allows nothing. A file that cannot be read keeps every job here.
Without the file nothing changes.

### Remove a tree from a node

A tree that reached a node by mistake is removed there with one command:

```console
$ slot nodes forget build-1 /srv/u/src/private
removed /srv/u/src/private (48213504 bytes freed)
removed /srv/u/.local/state/slot/ws/3fa2c1d09b7e4a11 (its link)
```

It runs the node's `forget` step through the node's `ssh` line and `slot_path` from the host
configuration file (`--cluster-file`, as for the other `slot nodes` commands). The path is the
tree as the node sees it: a workspace mirror `<ws-root>/<id>`, or a tree below one of the node's
`--same-path-root` directories. The node removes the tree and its records in the workspace root
(for a same-path tree, the workspace root's links to it), and prints each with the bytes freed.
It refuses, with exit 126 and `slot shell: refused: …`, a path with `..`, a root itself, any
other path, and a path with a symbolic link at or below its root. It refuses with exit 1 a tree
a queued or running job of its daemon uses (the job's working directory or a declared tree in
it, or the synced tree that holds them), and any tree while its daemon does not answer on
`--http`, since it cannot tell which trees are in use. The target directory a mirror built into
stays; the cache removes it as it removes any other.

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
because the server disk is full), the job does not run, and `slot` exits with 3 and prints
`slot: remote build on <host> did not start (…); the job did not run (set SLOT_REMOTE_FALLBACK=local
to run it on this machine's daemon instead)`. A build you sent to a server stays off this machine
unless you ask for it.

With `SLOT_REMOTE_FALLBACK=local`, `slot` sends such a job to this machine's daemon instead and
prints `slot: remote build on <host> did not start (…); running it on this machine's daemon`; the
job then waits for slots in the local pool like any other. From 0.8.0 to 0.38.0 this was the
default; before 0.8.0 the command ran directly, outside the pool. If the connection drops while
the job runs, `slot` prints `slot: connection to daemon lost` and exits with 3; the daemon
cancels the job.

A slow link gets the same treatment. A sync whose rsync reads and writes no byte for
`SLOT_SYNC_STALL` (default 60 s), or that runs longer than `SLOT_SYNC_LIMIT` (default 5 min), is
killed with its ssh, with `(sync killed: no byte moved for 60 s)` or `(sync killed: ran over
300 s)` in the line above. An rsync that someone else kills ends the same way. The job is never
sent to the server after a sync that did not finish.

## When the server's daemon does not answer

A server can be reached while its daemon takes no job: a daemon that was told to stop removes its
socket and lets its running and queued jobs end before it starts again, and a daemon that
reloads drops a job it had queued. `ssh <host> slot proxy` then ends before the job's process
started. When the daemon's socket is missing or refuses the connection, the server's
[`slot shell`](#restrict-the-key-to-slot) says so in one line and exits with 75:

```text
slot shell: daemon not answering: connect /run/user/1000/slot.sock: No such file or directory (os error 2)
```

The job waits: `slot` prints once

```text
slot: waiting for buildbox: its daemon does not answer (a restart or a drain); the job is sent again when it answers
```

and sends the same request again every 5 s until the daemon takes it. The tree is not synced
again, and the job runs once.

The wait ends after `SLOT_NODE_WAIT`: `35m` by default, given as seconds or a number with `s`,
`m` or `h`. The job then did not start, with `(its daemon did not answer within 35m)` in the line
of [When the server is unreachable](#when-the-server-is-unreachable), and `SLOT_REMOTE_FALLBACK`
decides as it does there. `SLOT_NODE_WAIT=0` turns the wait off: the call ends at once with
`slot: connection to daemon lost` and exit 3, as every release up to 0.38.0 did.

Four calls are never sent again:

- one whose job had started on the server: `slot` prints `slot: connection to daemon lost` and
  exits with 3;
- one that got SIGINT, SIGTERM or SIGHUP: it exits with 128 plus the signal;
- one whose ssh failed itself (exit status 255, which ssh keeps for a host it did not reach or a
  login it did not get): the call ends as it did before the wait;
- one the workstation's daemon handed to the node ([Hand queued jobs to a
  node](#hand-queued-jobs-to-a-node)): it does not wait, whatever `SLOT_NODE_WAIT` says. Its
  client ends with exit 3 at once and the daemon queues the job on the workstation again. A wait
  would count as the job's run time there. That client is known by `SLOT_SPILLED` together with
  `SLOT_PHASES_FILE`, which the daemon sets both. `SLOT_SPILLED` alone, set by a wrapper beside
  `SLOT_HOST` so that a failed start never runs on the workstation, waits like any other call:
  up to 0.40.0 its job ended with exit 3 at once while the node's daemon drained.

While a job waits, `slot status` on the workstation does not list it: the waiting call is not a
job of the workstation's daemon, and the node's daemon does not answer.

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

### What a go build wrote

A remote `go build -o <path>` or `go test -c -o <path>` that exits 0 leaves the file it built at
the same path in your checkout, with its mode and modification time, also when the path lies
under `target/` or another directory the sync leaves out:

```console
slot go build -o target/tools/mytool ./cmd/mytool
./target/tools/mytool --version
slot go test -c -o target/tools/pkg.test ./pkg
```

- **Which path.** The value of `-o`, written as `-o <path>` or `-o=<path>`; a relative path is
  read against the directory you called from (after `-C <dir>`). A path outside the checkout
  brings nothing back and prints nothing. An absolute path counts as inside the checkout only
  for a tree [mirrored at the same path](#mirror-at-the-same-path).
- **A directory.** When the path is a directory on the server before the job, or `-o` ends in
  `/`, slot lists it on the server before the job and after it and copies the files the job
  wrote: those that are new, or whose size or modification time changed. Any other file of that
  directory stays on the server. The top of the checkout is such a directory too
  (`go build -o . ./cmd/...` called there). Both listings are the server's, so no clock of this
  machine is compared with the server's. The times are compared to the second: a file written
  again with the same size within the second of its earlier time counts as not written.
- **Links.** A directory on the path that is a symlink in your checkout (`out -> /data/out`)
  stays a symlink: the file arrives where it points.
- **When.** After exit 0 only. The line is the same, for example
  `slot: fetched 1 artifact, 2.1 MB, in 0.2 s`. A listing or a copy that fails prints
  `slot: artifacts not fetched: <path>: …` and turns exit 0 into 1. A single file needs no
  listing before the job: when that listing fails, slot lists the path again after the job and
  copies the file when it is no directory.
- **Not touched.** `go run`, `go vet`, `go install`, a `go build` without `-o` and a `go test`
  without `-c` run as before: no listing, no copy. A `go test -c` without `-o` writes
  `<package>.test` into the working directory on the server, and it stays there: name the file
  with `-o` to get it back.
- **Off.** `SLOT_NO_PRODUCTS=1`, as for cargo.

## Crates are fetched once per Cargo.lock

Parallel cargo jobs on one server wait in turn on cargo's package-cache lock while one of them
downloads crates. Before a cargo job that builds (`build`, `check`, `clippy`, `test`, `bench`,
`doc`, `rustc`, `nextest`, `llvm-cov`), the server daemon compares the digest of the mirror's
`Cargo.lock` with the one it recorded for that mirror. When they differ it runs
`cargo fetch` once, under a lock per repository, records the digest, and prints
`slot: new Cargo.lock on this mirror: cargo fetch took 2.1 s`. The job then finds every crate in
the server's cargo home and downloads nothing.

The job's environment is not changed. Releases up to 0.39.1 set `CARGO_NET_OFFLINE=true` for
such a job. A compile cache keys every `CARGO_*` variable, so a cargo job shared no cache entry
with cargo inside a generic job (`slot sh -c 'cargo build'`), which gets no fetch and no
variable ([Cargo variables and the compile
cache](reference/environment.md#cargo-variables-and-the-compile-cache)). What the variable
guarded is now different: when a crate is missing after the fetch (the server's cargo home was
cleaned), the job downloads it and holds the package-cache lock for that long, where it failed
before; and a job whose `Cargo.lock` is out of date asks the registry's index. Set
`CARGO_NET_OFFLINE=true` yourself for a job that must not use the network: your value reaches
the job.

Like cargo on your machine, the fetch updates a `Cargo.lock` that is out of date, for example
after you add a path dependency. A job that passes `--locked` or `--frozen` itself gets
`cargo fetch --locked` instead, so it fails on the server as it would locally.

A fetch that fails (a `Cargo.toml` cargo cannot resolve, an unreachable registry, an out-of-date
`Cargo.lock` under `--locked`) or takes longer than 5 minutes is a job that did not start:
`slot: remote build on <host> did not start (cargo fetch failed …)`, exit 3, or this machine's
daemon with `SLOT_REMOTE_FALLBACK=local` ([When the server is
unreachable](#when-the-server-is-unreachable)). A mirror without a `Cargo.lock` gets no fetch,
and neither do commands that need the network themselves (`cargo update`, `add`, `install`,
`fetch`).

The digests live in `<ws-root>/.cargo-fetch/` on the server and go with their mirror when it is
evicted. Both features need this release of slot on the server and on the workstation.

## Git dependencies come from this machine

A `git+` source in a `Cargo.lock` may be reachable only from your machine: a private forge the
server holds no credential for, or no network to. The fetch above runs on the server, and only for
a cargo command slot sends itself; a wrapper's inner cargo call (`task check`, `make`) fetches on
the server too.

So before every remote job the client reads each `Cargo.lock` of the checkout that the sync sends,
and takes the `git+` sources and their locked revisions. `cargo metadata --locked --offline` in
that lock file's directory names the database entry (`<name>-<hash>`) cargo keeps for each
source, so an entry of another URL with the same name and revision is never taken. A revision
missing from this machine's cargo git database (`$CARGO_HOME/git/db`) gets `cargo fetch --locked`
there, with your credentials and outside the pool. The client then asks the server which of those
entries lack their revision, and copies only those into the server account's `~/.cargo/git/db`
over the same ssh and rsync:

```text
slot: git dependencies: 1 copied to buildbox in 0.4 s
```

cargo on the server then finds the locked revision in its database and does not fetch it. A job
whose entries are all on the server copies nothing and prints nothing. Only `objects/`, `refs/`,
`packed-refs` and `HEAD` of an entry cross: no `config` (it may hold a credential or an
`http.extraHeader`), no hooks, no `FETCH_HEAD`, no `objects/info/alternates` (it names a path
of this machine). The deny lists are read again before each entry is copied, and a source on them
keeps the job here
([above](#trees-that-never-leave-this-machine)).

The server's answer names entries only, never a path. An answer that names an entry the client
did not ask for, or a line it does not know, keeps the job here. On the server the copy goes into
the account's own `~/.cargo/git/db`, whatever `CARGO_HOME` the ssh session sets.

A step that fails does not stop the job: `slot: git dependency not here: …` names a revision this
machine could not fetch, and `slot: git dependencies not copied to <host>: …` a probe or copy that
failed (a server with an older slot answers no probe); the job then runs on the server as before.
The server needs this release of slot for `slot shell` to admit the probe (`slot __cargo-git`)
and the copy.

## What the job wrote or deleted comes back

After a job on the server, the tree here holds what the job left in its tree there, so a
generator that writes and removes files gives the same tree as on this machine:

- **Listed twice.** The client lists the job's tree on the server after the sync and again after
  the job, with an rsync that `slot shell` admits as a fetch. The times are the server's own and
  are only compared with each other.
- **Written.** A file or directory that is new after the job, or whose size or time changed,
  comes back (`rsync -a --checksum --update`), also under a directory `.gitignore` names. A file
  here that is newer than the server's stays.
- **Deleted.** A regular file that was in the tree before the job and is missing after it is
  deleted here when this machine's file is still the one the server had: it has the size and
  modification time it had when the sync sent it, or the size and modification time (to the
  second) of the server's file before the job, as a file that came back from an earlier job has.
  A directory the job removed is removed here when it is empty after that.
- **Kept.** A file the job deleted that changed here since the sync, or that was never sent and
  is not the server's copy, is kept, and so is anything a symbolic link of this tree leads to;
  each is named on a line of its own: `slot: return sync: kept notes.txt: changed here since the
  sync`.
- **Never touched.** Nothing under `.git`, `target` or `node_modules` is listed, copied or
  deleted, and no path that is a symbolic link here is written.
- **One line.** `slot: return sync from buildbox: 3 back, 1 deleted, 0 kept`: files that came
  back, files and directories deleted, files kept.

The return sync runs after every job that ran, whatever its exit code; a job that did not start
(exit 3) gets none. When a listing fails, for example on a server whose `slot shell` refuses it,
the client prints `slot: return sync skipped: could not list the tree on <host> before the job
(…)` and leaves the tree as it is; the job's exit code does not change.

`SLOT_RETURN_SYNC=0` turns it off. A wrapper script in front of `slot` that did a return sync of
its own and exports `SLOT_SHIM_PID` keeps it: then the client does none, so the two do not run
twice, unless `SLOT_RETURN_SYNC=1` is set. Such a wrapper's return sync never deleted a file;
remove it from the wrapper and set `SLOT_RETURN_SYNC=1`.

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
  `--same-path-root` too. When it is the directory cargo resolves by itself (`target` beside
  the workspace's root manifest), the job gets no `CARGO_TARGET_DIR`: the variable is part of a
  compile cache's key and would differ in every tree, so a crate built in one worktree would be
  compiled again in the next ([Cargo variables and the compile
  cache](reference/environment.md#cargo-variables-and-the-compile-cache)). When you set
  `CARGO_TARGET_DIR`, or `CARGO_BUILD_TARGET_DIR` or a `build.target-dir` of a cargo
  configuration on this machine names another directory, the job gets `CARGO_TARGET_DIR` with
  that directory's absolute path. So does a job for which `cargo metadata` fails here: slot
  then takes `target` beside the nearest `Cargo.lock`, which a cargo configuration might have
  moved, and names it to cargo rather than leave it to cargo. The job's record names the
  directory either way. Where the job gets no `CARGO_TARGET_DIR` it gets no
  `CARGO_BUILD_TARGET_DIR` either, also not one of the server daemon's own environment. A
  server whose own cargo configuration sets `build.target-dir` would build elsewhere than the
  record says: keep it off the server.
- A working directory the sync leaves out, such as `target/scratch/cli` or another git-ignored
  directory, is synced on its own, with its own `.gitignore` files and without its `target/`.
- A directory outside any git checkout (a scratch copy) is the tree itself, unless it is the
  same-path root or above it, lies directly under the same-path root, or holds a repository
  within two levels (`a/.git`, `a/b/.git`). Such a job does not run: `slot: remote build on
  <host> did not start (<dir> is not one repository's work tree (…); nothing is synced)`, exit 3.
  A folder that cannot be read is refused too. Start the job in the repository.

With `--sandbox`, the tree and the target directory are bound writable at their own paths; the
git directory is read-only. Files the command names outside the tree are not synced: a job that
reads them fails on the server, unless `.slot.toml` declares their tree (below).

### Sibling trees

A gate that reads a checkout beside its own (`../../specs`), or a build that runs
`cargo build --manifest-path ../driver/Cargo.toml`, needs that tree on the server too. Declare it
in `.slot.toml` at the repository root:

```toml
trees = ["../../specs", "../driver"]
```

Each entry is a path relative to the repository's root (the worktree's root in a linked
worktree), or an absolute path. In a linked worktree elsewhere on disk `../../specs` may lead
nowhere; a gate that finds its sibling through a variable declares the tree through the same one:

```toml
trees = ["${SPECS_DIR:-../../specs}"]
```

An entry may use `${NAME}` and `${NAME:-default}` (the default when `NAME` is unset or empty, as
in POSIX sh), and a leading `~/` for the home directory; a relative result is relative to the
repository's root. Nothing else is expanded: `$NAME` without braces, a `~` elsewhere and the text
of a default are taken as they are, and `${NAME-default}` or `${NAME:=default}` is an error. The
variables are those the job takes to the server: the ones its [toolchain](toolchains.md) sends
and the names in `SLOT_ENV` ([environment](reference/environment.md)). So the tree synced is the
one the job reads there; run `SLOT_ENV=SPECS_DIR slot task check`, or the entry and the job on
the server both get the default. An unset `${NAME}` without a default is a missing tree (below) that names the variable.

In same-path mode the client syncs each declared tree to its own
path on the server before the job, the same way as the job's tree: files git does not ignore,
without its `target/`, within the same stall and time bounds, and its git directory beside it so
that `git` reads work there. The job's record names them (`extra_trees`) and `slot status` shows
them under the job as `trees: …`.

- The server gets each tree as it is on this machine, uncommitted files included. It reaches none
  of the tree's remotes: a `git fetch` in the job fails there for want of credentials. Fetch here
  before the job, or keep such a gate here with `--here`.
- Each declared tree passes the checks of the job's own tree: it lies under the same-path root,
  is one repository's work tree or a plain folder, and neither it, its git directory nor a git
  dependency of its lock files is on the [never-sync list](#trees-that-never-leave-this-machine). It is
  also refused when one of its own remotes matches an `origin:` pattern of the list or of
  `remote-deny-origins`. A refused tree keeps the job on this machine's daemon with the rule and
  the tree as its reason (`never sync: /srv/u/specs (declared tree /srv/u/specs)`); nothing is
  synced.
- A declared tree that does not exist, or is no directory, ends a remote job:
  ``slot: remote build on <host> did not start (declared tree /srv/u/specs does not exist
  (.slot.toml `trees`)); the job did not run``, exit 3. An entry with a variable is named beside
  its path (``declared tree /srv/u/specs (`${SPECS_DIR:-../../specs}`) does not exist``). A local
  job runs, and carries that line as its reason, so no node takes it.
- Without `SLOT_REMOTE_SAME_PATH=1` a tree is mirrored under `<ws-root>/<id>`, where a relative
  path to a sibling leads nowhere: a job whose `.slot.toml` declares trees does not start there
  (exit 3) and says so.

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
output <id> <from>` every 2 s until the job ended. The file is read again when it changes, at the
next poll, without a restart: a node's new terms (ssh line, deadline, price, `spill`) are taken
in, a node new in the file joins, and a node taken out of it is dropped with its jobs. A node is
`Joining` until its first answer; a poll that fails or takes longer than 4 s is missed, and after
three misses in a row the node is `Gone` (reason `Lost`) with its last row; the next answer makes
it `Ready` again. A key of the file that the shape does not declare is refused, and the daemon
then polls no node.

## One registry of build nodes

The host configuration file is the one list of a host's build nodes: the daemon, `slot
status`, the web view and `slot usage` read their nodes from it. A node a job can run on is a
`[remote.<name>]` entry; a node that is not one is polled by nobody, and its jobs appear
nowhere. Two keys route a repository's jobs:

```toml
default_node = "build-1"

[remote.build-1]
ssh = "ssh -o BatchMode=yes build-1.example"
repositories = ["repo-a"]

[remote.build-2]
ssh = "ssh -o BatchMode=yes build-2.example"
spill = false
repositories = ["repo-b", "repo-c"]
```

- `repositories`: the repositories whose jobs go to the node, by the name `slot status`
  prints (the directory of the checkout's common git directory, without `.git`; a linked
  worktree has its main repository's name). A repository is listed under one node at most.
- `default_node`: the node of every repository no entry lists.

`slot nodes` reads and writes the file (`--cluster-file`, else `SLOT_CLUSTER_FILE`, else
`$XDG_CONFIG_HOME/slot/cluster.toml`, the file the daemon reads):

- `slot nodes list` prints each node: name, state as the daemon sees it (`-` when the daemon
  does not answer), spill, whether it is the default, and its repositories.
- `slot nodes route [<dir>]` prints the node a job in `<dir>` (default: the working directory)
  goes to, as shell assignments. It reads only the file: no daemon and no network.

  ```sh
  $ slot nodes route ~/src/repo-b
  SLOT_NODE=build-2
  SLOT_HOST=build-2.example
  SLOT_SSH='ssh -o BatchMode=yes'
  SLOT_REMOTE=slot
  SLOT_NODE_END=
  export SLOT_NODE SLOT_HOST SLOT_SSH SLOT_REMOTE SLOT_NODE_END
  ```

  `SLOT_NODE` is the entry's name, `SLOT_HOST` the last word of its `ssh` line, `SLOT_SSH` the
  words before it, `SLOT_REMOTE` its `slot_path` (`slot` when absent) and `SLOT_NODE_END` its
  `deadline` in Unix seconds, empty without a deadline, so a value from an earlier `eval` does
  not stay set. With no entry
  for the repository and no `default_node`, or a `default_node` without an entry, it prints
  one line naming the file and exits 3. A wrapper in front of `slot` sources it and names no
  node of its own: `eval "$(slot nodes route)" && exec slot "$@"`.
- `slot nodes add <name> --ssh <cmd> [--slot-path <path>] [--deadline <rfc3339>]
  [--price-per-hour-cents <n>] [--spill true|false] [--repository <name>]… [--default]` writes
  the entry at the end of the file. A repository listed under another node moves to the new
  one (a node left without repositories loses the key); `--default` makes it `default_node`.
- `slot nodes remove <name>` takes the entry out; its repositories then go to `default_node`,
  and removing the default node clears `default_node`.

`add` and `remove` keep the rest of the file as it was, comments and order included, and refuse
a file that does not parse. The daemon takes the change in at its next poll, within 5 s.

When a poll shows a node's job ended for the first time, the daemon appends it to
`node-jobs.jsonl` in its state directory, beside `jobs.jsonl`, once (also across a restart),
with `node` set to the node's name and the id the node gave it. `slot usage` and
`GET /api/usage` read both files; `slot usage --by node` and the **node** grouping of the web
view's usage table show each node's share, `local` for this host's own jobs.

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
and runs here. A command that itself exits 3 is run a second time here too. A job handed to a node
whose daemon does not answer does not wait for it, whatever `SLOT_NODE_WAIT` says ([When the
server's daemon does not answer](#when-the-servers-daemon-does-not-answer)): its client ends
with exit 3 at once and the job is queued here again.

A job that runs or waits here while a node takes jobs carries the reason in `local_reason`
(`/api/state`), and `slot status` prints it under the job's line as `  local: <reason>`:
`local requested` (`SLOT_NO_REMOTE=1`, or a command `.slot.toml` lists as local), `cache rule`
(the working directory or an argument under the caller's `~/.cache`), `nested job`, `gated job`,
`remote job`, `sent back by a node`, `slots free here first` (it got slots here before a node
took it), or `sync failed: <first line of the error>` from a remote-mode client whose start
failed and that ran the job on this machine's daemon instead (`SLOT_REMOTE_FALLBACK=local`). A wrapper sets its own reason with
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
one entry directly under the account's `~/.cargo/git/db`, never read back; the entry may not be a
symbolic link, and `-l`, `-L`, `-k`, `-K`, `-R`, `--copy-unsafe-links`, `--safe-links`,
`--force`, `--inplace` and every `--delete…` option are refused for it) and the read-only `state` step (`slot state`: the server daemon's own row of
`/api/nodes` and its jobs, read from `--http`, for the
[node poll](#show-the-server-in-the-web-view)) and the read-only `output` step (`slot output <id>
[<from>]`: the kept output of one job from its chunk `<from>` on, read from `--http`) and the
`forget` step (`slot forget <path>`: removes one mirror or same-path tree, [Remove a tree from a
node](#remove-a-tree-from-a-node)). Anything else
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
