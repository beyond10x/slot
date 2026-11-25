# Changelog

## Unreleased

## 0.37.0 — 2026-11-25

- `slot cache refused [--since 24h] [--by repo|project] [--json]` lists, per repository, the
  compiler calls the sccache servers refused to cache by reason, with hits and misses, from
  `cache.jsonl` and `jobs.jsonl` in the state directory. A minute's calls go to the jobs that ran
  in it with compile-server CPU: to one repository exactly, or split across several by each job's
  compile-server CPU rate times its time in the minute (shared, its own column). Minutes no job
  explains get a line of their own, and every reason a line with its remedy.
- `contrib/node/` provisions a cloud build node and reports where a running one differs from it.
  `provision.sh` (root on the node) sets up the instance store array, the job tmp filesystem,
  the binds of the same-path mirror roots, packages, the build user, the units, the build key and
  the slot release; `push.sh` (workstation) copies the workstation's `/usr`, `/etc` entries, rust
  toolchains and cargo tools to the node. Both have `check` (one `drift:` line per difference,
  exit 1) and `apply`. Everything private is in a config file outside the repository
  (`node.conf.example`). The node's slots follow the CPUs found. A unit rebuilds the instance
  store at boot, so a stop and start no longer leaves bind lines in `/etc/fstab` that point at
  an empty disk. `task check` runs `contrib/node/test.sh`.
- `provision.sh apply` installs the slot release into the instance store and the build user's
  home again, and reads every installed path back: a failed install exits 1 with a `failed:`
  line instead of `changed:`.
- `push.sh check` compares the copied home directories file by file (name and size): a file
  missing or of another size on the node is drift, a file a node job added there (a cache) is
  not. `push.sh compare HERE NODE` runs the comparison on two digest files.
- `push.sh apply` uses an ssh control connection of its own and ends it on exit. A control
  connection shared through the ssh config stayed logged in to the node's setup login after
  `provision.sh apply` had removed the setup key.
- `provision.sh apply` without `--open` ends every login of this boot made with the setup key that
  still runs: its logind session (every process the login started) or, without one, its
  connection; found by the key's fingerprint in the sshd log. `check` reports them.
- `provision.sh apply` writes the approved `HOME_COPIES` to `/etc/slot-node/copies`;
  `push.sh apply` prints its copy list and refuses to copy when it differs from the node's.
- `provision.sh` runs under umask 022, so a bundle's umask 077 no longer leaves `/etc/slot-node`
  at mode 700, where the build user could not read the copy list and `push.sh apply` refused to
  copy. `check` reports the directory at any mode but 755 and `apply` sets it.
- The node's line for a git or ssh call that asks for a credential names `slot --here` only for
  the step that reads a credential while it runs, never for a whole task, and sends a step that
  needs docker or a local service to `.slot.toml` `local`. Before, it offered `--here` for the
  step, and callers sent whole tasks back to the workstation with it.
- The node's cargo config turns incremental compilation off. The compile cache refuses every
  incremental rustc call; on one node 2,929 calls in 3.7 hours were refused for that reason. A
  job that sets `CARGO_INCREMENTAL` itself still overrides it.

## 0.36.0 — 2026-11-24

- Products copied back from a node no longer pile up in the local target directory. After a
  remote cargo job's products are copied back, the client removes, in each directory it copied
  into, every older file of the same target: a file named `<stem>-<16 lowercase hex>`, with or
  without an extension, for a stem it just copied, whose hash is not one it just copied.
  Directories and every other name stay, and nothing is removed for a product that did not
  arrive. Before, each rebuild of a test target under a new hash left the old binary behind, and
  the private target directories of worktrees that built on a node grew by several GB each. The
  products line names what went:
  `slot: fetched 3 artifacts, 41.2 MB, in 0.9 s; removed 2 superseded files, 38.4 MB`.
- `SLOT_NO_PRODUCTS=1` turns the copy-back off for a job: the command runs on the node unchanged
  (no `--message-format` added) and the client prints `slot: artifacts not fetched:
  SLOT_NO_PRODUCTS is set` where it would have copied.
- A changed source is never taken as built on a build node. The sync gave each file on the node
  the modification time it had on the workstation (rsync `-a`). cargo decides by those times, so
  an edit made before a build the node ran on the old content, and synced after that build, was
  older than it: cargo reported the crate fresh and ran the old code. Touching the file on the
  node did not help, because the next sync set the workstation's time again. The sync now leaves
  the node its own times (`--no-times`, with the content comparison `--checksum` as before, and
  the same for a same-path tree's extra directory): a file whose content changed gets the time
  the sync wrote it, and an unchanged one keeps its time on the node. Seeding a new worktree's
  target directory from a sibling's now sets the times of the new mirror after the copy: a
  source file with the sibling's bytes takes the sibling's time, every other one a time after
  every file of the copy, so cargo rebuilds each crate whose sources differ from the sibling's.
  The job's stderr counts those files. The copy stays a copy (`cp -a`), never a hard link into
  the sibling's target; `slot serve --seed-copy copy` takes a byte copy where the filesystem has
  no reflinks (default `reflink`: no seed there, as before).
- A job's record says where its time went. A remote job carries `sync_ms` and `sync_bytes` (the
  tree to the node, as its client measured it) and `back_ms` (the cargo products and `--fetch`
  paths back); a cargo `test`, `nextest` or `bench` job carries `compile_ms` and `test_ms`, split
  where its output shows the first test binary (cargo's `Running …`/`Doc-tests …` line, nextest's
  `Starting N tests across …` line or cargo's `build-finished` message); a remote cargo job
  carries `node_target_dir` and `seeded_from` (the sibling workspace it was seeded from, `kept` or
  `empty`). Each field is absent where it does not apply and in older lines, never 0. The node's
  record gets the sync with the request; the back time arrives after the job in a second request
  (`amend`) and is appended to the node's job log as `{"amend":<id>,"back_ms":<ms>}`, which
  readers of job lines skip and the daemon applies when it reads the log back. A job the daemon
  hands to a node gets the client's figures from the file in the new `SLOT_PHASES_FILE`. The
  `start` frame names `seeded_from`.
- A job on a build node gets a width when it starts: the node's free CPUs at that moment (CPUs
  less the busy ones of the last seconds) divided by the jobs that started in the last 5 s, this
  one counted, at least 1 and at most the node's CPUs. A job is on a node when it was synced from
  another host. It gets the width as the new `SLOT_WIDTH` (for a Playwright or Vitest config),
  `RUST_TEST_THREADS`, `NEXTEST_TEST_THREADS` and `GOMAXPROCS` (a lower count the caller set is
  raised to it) and go's `-p`; while `slot job-parallel` is on, a lower bound per job
  (`CARGO_BUILD_JOBS`, `MAKEFLAGS=-j`) is raised to it. The job record carries it as `width`.
  Before, every job got a quarter of the pool as test threads (16 on a node of 64 CPUs). Local
  jobs keep `--test-threads`, the bound and the caller's counts.
- `slot warm [DIR]`: after a merge to `main`, the checkout tells the daemon, and the daemon builds
  that tree's tests (`cargo test --no-run`, or `warm = "<command>"` in `.slot.toml`; `warm =
  false` switches it off) as a warm-up job without the client. It starts one only for a
  repository a job built on it in the last 24 hours, one at a time per repository. The job is
  queued at `low` over any priority rule and never promoted; a job of a session that still waits
  for its slots 50 ms after it queued cancels every running warm-up, which ends `cancelled` with
  the new `cancel_reason` `preempted by job <id>`. On a node a warm-up that ends with exit 0
  leaves its target directory the most recently used of the repository, the one the next new
  worktree is seeded from. A daemon hands a warm-up to a node as `slot run --warm`; it goes only
  while no other job waits for a node. The request and the job record carry the new `warm`.

## 0.35.0 — 2026-11-24

- A job's output reaches the caller whole when another process sharing the caller's stdout has
  made it non-blocking (a test runner or bundler in the same pipeline). Before, the client gave up
  on the first `EAGAIN` and the rest of that output block was lost mid-line while later lines
  arrived.
- A node reports what only it knows, in its row of `GET /api/nodes`: the machine's boot
  (`booted_at`), the poweroff systemd has scheduled (`poweroff_at`), the CPUs busy on the whole
  machine and its load (`host_cpu_milli`, `load1`; `/api/state` adds `host_busy_cores` and
  `load1`), and the jobs it ended in the last 60 minutes and per hour of the last 25, by result
  (`ended_hour`, `ended_hours`: `ok`, `test`, `timeout`, `infra`, `cancelled`), counted from its
  whole job log. The web view's node cards show those counts for the last 60 minutes and today,
  the same windows on every node (before: the local daemon's kept jobs against a node's last 30),
  and the whole host's CPU and load (before: the pool's CPU only, 0.0 on a busy host). A remote
  node's lease ring counts to its scheduled poweroff, and its card is red when that lies more
  than 5 minutes from the deadline of the cluster file or when none is scheduled. The cloud cost
  runs from the machine's boot to now (before: from the daemon's join to the deadline, so a node
  that outlived a stale deadline cost nothing more). Failed jobs are split by cause in the header
  and on the cards: test, timeout (124), infra (3, 125, 127). UXF-039 to UXF-043.
- The output of a node's job opens in the web view: `GET /api/jobs/<id>/output?node=<name>` reads
  it from the node with the new read-only `slot shell` step `slot output <id> [<from>]` (the node
  daemon's `GET /api/jobs/<id>/output-tail?from=<n>`), every 2 s until the job ended. Before, the
  pane said the output stayed on the node. UXF-044.
- Web view with build nodes: a node job's cell holds its id (up to 7 digits) and its node on a
  line below, clear of the tool column (UXF-050). Panels that count this host only say so ("slots
  in use on this host", the Cores charts, "Share of the pool on this host"); the Running line
  splits node jobs from this host's; Finished keeps the jobs that ended last, so a node's jobs
  appear there (UXF-051). A job's session is its title wherever the daemon resolved one (also
  from `api/usage` grouped by session), "no session" when it has none, and the cluster page's
  job list has a session column (UXF-052). Below 1100 px the cluster page stacks instead of
  scrolling sideways; the layout gates hold it at 800x600 and 1024x768 (UXF-053). With an empty
  queue the endpoint panel lists each node's running jobs and busy slots (UXF-054).
- Remote mode's safety rules are in the `slot` binary, so they hold however it is called (with
  `SLOT_HOST` already set, through a wrapper or without one):
  - The never-sync list: a file of absolute path prefixes (`never_sync_file` of `cluster.toml`,
    default `$XDG_CONFIG_HOME/slot/remote-deny`, `#` comments). A job whose working directory,
    checkout root, git directories or a tree an argument names (`--manifest-path`, `-C`, `--dir`,
    `--directory`) lies under a listed prefix, or whose tree is `/` or holds the home directory,
    is never synced: it runs in the local pool with `local_reason` `never sync: <prefix>`, which
    also keeps the daemon from handing it to a node. Prefixes match whole path components, as
    written and with symlinks resolved. An unreadable list keeps every job local.
  - A variable whose name looks like a credential never crosses, even when `SLOT_ENV` lists it.
    The list grew by `PASS`, `SSH_`, `AWS_SECRET` and the words `PAT`, `DSN` and `KEY`; the
    client names each name it held back on stderr, never the value. Before, any name in
    `SLOT_ENV` crossed.
  - Symlinks stay links: before the sync the client lists the tree with the same `rsync`
    (`--list-only`), counts links that leave the tree on the sync line (`2 symlinks leave the
    tree (sent as links)`), and refuses the sync, running the job here, when that `rsync` would
    send such a link's target in its place (a `PATH` wrapper with `--copy-unsafe-links`).

## 0.34.0 — 2026-11-24

- `slot --version` and a node's `slot_version` name a build that is not of its release tag:
  `0.34.0+g31e76f1`, with `.dirty` when tracked files changed. A build of the tag (or a CI run
  started for it) prints the bare version.
- Same-path remote mode keeps a tree's own path on the server only for a tree under the client's
  home (`SLOT_REMOTE_SAME_PATH_ROOT` names another directory); any other tree (a runner's work directory elsewhere) goes to the server's
  workspace root. Before, the server's `slot shell` refused such a tree's sync (`rsync path … is
  outside`).
- A node's job keeps the id its node gave it: `GET /api/state`, the job events, `slot status`
  and the web view show that positive id, with the node in `node` and `from_node: true`. Before,
  the daemon moved such ids below 0 (`-1000000007499`), and only two tables of the web view
  rewrote them. The web view names a job by node and id together, so a node's job and a local
  job of the same id are two rows; labels read `output of job 7089 on build-1d`; the output pane
  of a node's job says its output stays on that node. `slot status` prints `#7089 on build-1d`.
  UXF-038 holds it in the UX gate.
- The daemon reads the host configuration file again when it changes, at the next node poll: a
  node's deadline, price, place and machine type follow the file without a restart. Before, a
  lease extended in the file kept showing the old deadline until the daemon restarted.

- `contrib/rustc-gate`, set as `rustc-wrapper`, refuses a compile outside a slot job (no
  `SLOT_JOB`) with exit 3 and the `slot run -- …` command that replaces it; `slot --local`,
  `SLOT_SHIM_OFF=1`, `SLOT_EDITOR=1` (rust-analyzer) and queries such as `rustc -vV` pass.
  `contrib/rustc-gate.test.sh` runs in `task check`.
- The PATH shim (`contrib/slot-shim`) sends every `cargo` subcommand through slot except the ones
  that compile nothing (`fmt`, `metadata`, `tree`, `update`, `clean`, `add`, … and `mutants`).
  Before, `cargo run` and aliases such as `cargo xtask` compiled outside the pool.
  `contrib/slot-shim.test.sh` checks the routing and runs in `task check`.
- `slot --here <command>` keeps the job in this host's pool, never on a build node, for a step
  that needs this host's credentials while it runs (a clone, a fetch, a push): the client skips
  remote mode (`SLOT_HOST`) for the call and the job carries the reason `here: needs this host`.
  Unlike `--local` it still goes through the daemon's queue and slots.
- A job that runs or waits on this machine while a remote node is configured names why: the
  job record's new `local_reason` (`/api/state`, job events, UI types) and a `  local: <reason>`
  line under the job in `slot status`. Reasons: `local requested`, `cache rule`, `nested job`,
  `gated job`, `remote job`, `sent back by a node`, `slots free here first`, `sync failed: <first
  line>` (sent by a remote-mode client in the new request field `local_reason` when it falls back
  to the local daemon), or the text of `SLOT_LOCAL_REASON`. A job with a named reason is not
  handed to a node.
- Eviction no longer deletes a target directory a job still uses. It keeps every target directory
  in the tree of a queued or running job (git's top level of its working directory, else the
  working directory; nested ones such as `target/ignite-test/rt` or `driver/target` too), one equal
  to, inside or holding the target its `CARGO_TARGET_DIR` names, one with a `.cargo-lock` a process
  holds locked (`/proc/locks`), and every target directory in the tree of any job that ran in the
  last hour, whatever the idle keep: the steps of one long task run as several jobs in a row.

- Spill-over: the daemon hands every queued local job to a remote node of the host
  configuration file while the node is Ready, before its deadline and has free slots by its last
  answer (less the jobs handed to it in the last 15 s), oldest first. The job's process is
  `slot run -- <command>` in remote mode with `SLOT_HOST`, `SLOT_SSH` and `SLOT_REMOTE` from the
  node's `ssh` line and `slot_path` and `SLOT_REMOTE_SAME_PATH=1`; it holds no local slot, its
  output and exit code pass through, and its record has `node` set. Exit 3 (sync, ssh or
  connection failed) puts the job back in the local queue once. Keys: `spill` per node (default
  `true`) and `spill_after_s` (default 0) at the top of the file. Never handed: nested jobs, `SLOT_NO_REMOTE=1` jobs, a
  remote client's job and a gate's hold. `slot status` prints `spill: N jobs to <node>`.

- The daemon shows remote build nodes: each `[remote.<name>]` table of the host configuration
  file (`$XDG_CONFIG_HOME/slot/cluster.toml`, `slot serve --cluster-file`, `SLOT_CLUSTER_FILE`)
  is polled every 5 s over ssh with the new read-only `slot shell` step `state`, which prints the
  node daemon's own row of `/api/nodes` and its jobs. `GET /api/nodes` lists each node (kind
  Remote; Joining until its first answer, Gone with reason Lost after three missed polls, a poll
  bounded at 4 s); `GET /api/state` and the job events carry its jobs with `node` set and an id
  below 0. The cluster page shows the node's card and jobs; its job list is ordered by queue
  time.
- Same-path mirrors: with `SLOT_REMOTE_SAME_PATH=1` the client syncs the tree to its own absolute
  path on the server and the job runs in the same directory as on the client, so absolute paths
  into the tree and compile-cache keys that include the working directory match. The server
  admits it only below a `--same-path-root DIR` given to `slot serve` and `slot shell`
  (`SLOT_SAME_PATH_ROOTS`, `:`-separated); outside them `slot shell` refuses the sync and the
  daemon the job. `<ws-root>/<id>` links to the tree, so sibling seeding and eviction keep their
  ids; the sandbox binds the tree writable at its path. Without the variable nothing changes.

- A same-path job runs like a CI runner's: its tree with uncommitted and untracked files, its git
  directory, its own target directory, at the client's paths. The git directory is synced at its
  own path (for a linked worktree the repository's common one, which holds the worktree's; one
  sync of it at a time, without `*.lock` files), so `git` works in the job. Cargo builds into the
  client's target directory: `CARGO_TARGET_DIR` when the caller sets it, else the one
  `cargo metadata` names for the command's manifest (`--manifest-path` included), never
  `<cache-root>/target/<id>`; the paths cargo prints and `CARGO_BIN_EXE_*` hold on both machines,
  and products come back to the same paths. A working directory under a path the sync leaves out
  (`target/…`, a git-ignored directory) is synced on its own, with its `.gitignore` files and
  without its `target/`. A directory outside any git checkout is the tree. The sandbox binds the
  target directory writable.

- A remote `cargo build`, `test`, `bench` or `rustc` brings its products back into the local
  target directory, at the same relative paths with modes and mtimes: every executable of the
  workspace's own packages (binaries, unit and integration test binaries, benches, examples)
  and the files of `cdylib` and `staticlib` targets; never a dependency's artifacts. The client
  adds `--message-format=json-render-diagnostics` on the server when the caller named no
  format, reads cargo's `compiler-artifact` messages and takes those lines out of stdout, so the
  caller sees cargo's usual output. A build fetches after exit 0, a test or bench run whatever
  its exit; one rsync copies every path and the client prints
  `slot: fetched N artifacts, X MB, in T s`. A failed copy turns exit 0 into 1. `slot shell`
  admits a fetch of several paths, each under the workspace root or the cache's `target/`.

- Fetch once: before a remote cargo job that builds, the server daemon runs
  `cargo fetch` in the mirror when its `Cargo.lock` digest differs from the one recorded
  for that mirror (`<ws-root>/.cargo-fetch/<id>`), under a lock per repository, and then runs
  the job with `CARGO_NET_OFFLINE=true`. Like cargo on the workstation, the fetch updates a
  `Cargo.lock` that is out of date (a path dependency just added) instead of failing; a job
  whose own options include `--locked` or `--frozen` gets `cargo fetch --locked`, so it fails
  as it would locally. Parallel jobs no longer wait on cargo's package-cache
  lock while one of them downloads. A fetch that fails or runs over 5 minutes sends the job back
  to the client, which runs it on its own machine's daemon (a new `fallback` frame, sent only to
  a client whose request sets `fetch_once`).
- The Claude Code hook lets a task runner (`task`, `make`, `just`, `sh`, `bash`) run with
  `SLOT_ORCHESTRATOR=1` in front. It is for a gate whose own steps call `slot`: run as one
  `slot` job, the whole gate was capped at 30 minutes and its steps shared that job; with
  `SLOT_SHIM_OFF=1` a step that compiled without `slot` ran outside the pool. With the marker each
  step is its own pool job, the gate has no cap, and the shims stay on. A compiler or test runner
  behind the marker is still refused.

## 0.33.1 — 2026-11-24

- Remote mode's sync compares every file by content (`rsync --checksum`), not by size and
  modification time. A file changed at the same size and time was skipped, and through
  `--copy-dest` a sibling mirror's file of the same size and time was taken as it was: a job then
  built an older version of a source file and could pass. The cost is reading every file on both
  sides: on a tree of 10,000 files (99 MB) a sync with nothing to send took 0.33 to 0.68 s instead
  of 0.13 to 0.36 s with the files in the page cache, and up to 30 s on a heavily loaded machine.
- The sync line names what was sent: `slot: synced to <host> in … s, … bytes sent, at 1a2b3c4 +
  3 changed files`, the commit of `HEAD` and the lines of `git status --porcelain` (tracked
  changes and untracked files git does not ignore). Without git it says the sources are not
  named.

## 0.33.0 — 2026-11-24

- `slot serve --sandbox` runs every job under bubblewrap (`bwrap`): the whole filesystem is
  read-only, in a pid and IPC namespace of the job's own. Writable are only the job's checkout
  (its synced workspace, or the git checkout of a local job), its target directory, a private
  `/tmp` (`TMPDIR=/tmp`), cargo's `registry/` and `git/` with their lock files, the Go caches and
  each `--sandbox-write <path>`. Other workspaces, other jobs' target directories, the user's
  runtime directory and the daemon's socket are hidden; the network is the host's. A job can no
  longer delete, replace or install a program in `~/.cargo/bin`, `~/.rustup` or `/usr` that
  another build needs. Without a `bwrap` that can create the sandbox the daemon does not start.
  `SLOT_SANDBOX=bwrap` is set in the job. Off by default; meant first for build nodes
  (`dnf install -y bubblewrap` in the boot script).

- `slot serve --sandbox --sandbox-fixed-paths` mounts each job's checkout at `/work/src` and its
  target directory at `/work/target`, so every worktree of a repository builds at the same paths
  (`CARGO_MANIFEST_DIR`, the working directory, `CARGO_TARGET_DIR`). Off by default: such a job
  runs without a rustc wrapper, because the compile cache server outside the sandbox cannot read
  sources at those paths.

## 0.32.0 — 2026-11-24

- Remote mode seeds a new worktree's mirror from its siblings. Before each sync the client asks
  the server for other mirrors of the same repository (same normalized `origin` URL), through a
  new step `slot __siblings` that `slot shell` admits, and passes up to four of them to rsync as
  `--copy-dest`: unchanged files are copied on the server, changed ones sent as deltas against the
  sibling's. The first sync of a worktree used to send the whole tree; over a link of about
  1 MB/s a 145 MB tree took more than 16 minutes. A server whose slot does not know the step gets
  the plain sync. `slot shell` takes `--copy-dest` only as `../<id>` of a sync, and `--stats`.

- A remote sync that reads and writes no byte for `SLOT_SYNC_STALL` (default 60 s), or runs
  longer than `SLOT_SYNC_LIMIT` (default 5 min), is killed with its ssh, and the job runs on this
  machine's daemon. An rsync killed from outside also sends the job to the local daemon. Before,
  a sync over a stalled link held the job for as long as the link stayed open.

- Every remote sync prints one line with its bytes and seconds, for example `slot: synced to
  buildbox in 0.4 s, 81234 bytes sent, 1 sibling mirror as base`.

- Two clients of one worktree on one workstation sync it one after the other: the second waits
  for the first (a `flock` in the runtime directory, at most `SLOT_SYNC_LIMIT`) and its line says
  how long. Two rsyncs into one mirror at once had ended one of them with exit 23.

## 0.31.1 — 2026-11-24

- The UX gates' accessibility probe waits until every finite animation on the page has ended
  before axe-core runs. The 0.31.0 release gate measured the colour of cluster-view rows while
  they faded in and failed on contrast; 0.31.0 has no published release, and 0.31.1 carries its
  changes.

## 0.31.0 — 2026-11-24

- The web UI opens on a cluster view: headline numbers, the queue, a card per node with its
  slots, CPU over 60 s, memory, free disk and the checkouts with jobs on it, jobs moving from the
  queue to their node, and the newest jobs with their node. Selecting a node shows only its
  jobs; Escape shows all. A dark and a light theme; with reduced motion nothing moves but fades.
  The job tables, charts and output are unchanged at `#jobs`, linked from both views.
- `GET /api/nodes` answers one row per node in the names of the cluster specification
  (`node_id`, `kind`, `state`, `cpus`, `slots`, `slots_in_use`, `memory_gib`, `disk_free_gib`,
  `cpu_busy_milli`, `joined_at` and more); today the one row is the local machine. Every job in
  `/api/state` and in the job log names its `node` (`local`). Against an older daemon the page
  builds the local node from `/api/state`.

## 0.30.0 — 2026-11-23

- `slot evict-below [GB|default] --idle <duration>` sets on the running daemon how long eviction
  keeps a target directory after its last build, from 5 minutes to 7 days; 2 hours until it is
  set. After a host restart every session rebuilt, about 100 target directories were under
  2 hours old, and eviction could free 8 GB. It applies at the next look, is kept in
  `limits.json` (`evict_idle_s`) only when set, and is `evict_idle_s` in the body of
  `PUT /api/evict-below`, where a field left out stays as it is and `null` forgets it.
  `slot evict-below` prints it; `default` forgets it with the mark.

- A target directory whose deletion failed, such as a managed worktree whose session lease is
  live, is left out of eviction for an hour, and the journal says so once. Before, it was
  planned first again every minute and eviction freed nothing.

## 0.29.0 — 2026-11-23

- `slot evict-below [GB|default]` sets the eviction mark on the running daemon: the free space
  below which it deletes idle target directories. `--evict-below` changed only at a full
  restart, which drains the pool. The mark replaces `--evict-below`, is still raised to
  `--min-free-disk`, leaves the wait for disk unchanged, and 0 turns eviction off; `default`
  forgets it. A new mark makes eviction look at once. It is kept in `limits.json` only when set;
  the API is `GET` and `PUT /api/evict-below` with `evict_below_gb`.

- Eviction keeps only what a job uses: a target directory a queued or running job builds into,
  or one in the checkout such a job runs in. Before, any queued or running job of a repository
  kept every target directory of that repository, so on a host with many sessions at work
  nothing of their repositories was deleted.

## 0.28.0 — 2026-11-23

- The compile cache chart's legend gives the share of the last 24 hours beside the newest minute's
  (`26 % · 24 h: 41 %`): a minute swings from 0 to 70 %. The daemon serves the sum at `GET /api/cache-day`.

## 0.27.0 — 2026-11-23

- The CPU limit for starts also holds on CPU pressure: `slot cpu-limit 17 --pressure 80` holds a
  queued job while `some avg10` of `/proc/pressure/cpu` is at 80 % or more and the floor's jobs
  run, and lets it start when the value falls under the mark. On a saturated host the CPU time of
  the pool stops rising at its share while the host keeps getting slower; the pressure keeps
  rising. On one host a mark of 12 CPUs held nothing at a load of 208 on 20 CPUs, because the
  pool read 11.5 CPUs. Off by default; it needs a CPU mark, is kept in `limits.json` only when
  set, and keeps the bound of 10 minutes, its pacing and the pass for short jobs. `slot cpu-limit`
  prints the mark and the current pressure, `slot status` names the mark, and the API field is
  `cpu_pressure_max`.

## 0.26.0 — 2026-11-23

- Eviction no longer deletes a target directory that a process or a service uses. It deleted one
  that a systemd user unit ran its daemon from while a command executed from it; the next run
  failed with exit 127. Before it deletes, eviction now reads `/proc` once per pass and keeps a
  directory that holds a process's executable, working directory, or an open or mapped file, and
  one that an `ExecStart` line of a user unit names, whether or not the unit runs. The journal
  says which and why.

- A job whose command is predicted to run 30 s or less is not held by the CPU limit for starts.
  On one host 51 of 169 jobs in 30 minutes ran under 30 s; they waited 22,180 s in the queue,
  435 s each, for 176 CPU-seconds of work in all, most of it the CPU hold's bound of 10 minutes.
  The prediction is the 90th percentile of the command's last runs in the same repository, with
  at least 3; a command without history is held as before. The client says why the job passed,
  and the census log gets a `cpu_hold_short` line.

## 0.25.2 — 2026-11-23

- With no daemon answering, `slot <command>` no longer runs the command outside the pool and
  exits 0. It waits up to 60 s for the socket, as during a restart, then exits with 125 without
  running the command and names the opt-in. A program that started slot with a cleared
  environment ran every build unpooled and unrecorded this way. `SLOT_LOCAL_FALLBACK=1` restores
  the old behaviour; `slot --local` still runs one command without the daemon. A job dropped by
  an upgrading daemon that does not come back within 10 min also ends with 125 now.

- Without `XDG_RUNTIME_DIR`, the client and the daemon look for the socket in `/run/user/<uid>`
  when that directory exists, where systemd puts it, before the temporary directory. The ssh
  control socket of remote mode follows the same directory.

## 0.25.1 — 2026-11-23

- The CPU limit for starts no longer lets a job through as "held as long as a job is" for time it
  waited for disk or memory. A job held for the host's CPUs and then for disk kept the clock of
  its first CPU hold; when the disk freed, it was past the 10-minute bound and started whatever the
  host's CPUs did. On one host 5 jobs started that way at once after a disk hold of about 44
  minutes, held 812 to 863 s by that clock. The clock now starts again while a job waits for disk
  or memory.

- Held jobs that reach the CPU hold's 10-minute bound together start one by one, 30 s apart,
  not all at once: with the mark lowered on a busy host, 34 jobs held within 5 minutes would have
  started within 5 minutes of each other. Each one waiting for its turn says "held as long as a
  job is; one such job starts every 30 s".

## 0.25.0 — 2026-11-23

- The host's busy CPUs no longer read more than the host has. The daemon divided the CPU time
  between two `/proc` scans by the time between two ticks of its 1-second clock; under load the
  scan starts late on a blocking thread, so a sample covered up to twice the time it was divided
  by. On a host of 20 CPUs, 35 of 2,160 records of 10 samples read over 20, the highest 37.0, all
  at a load of 108 or more; the web view scaled its CPU chart to them ("37 cores") and the CPU
  limit for starts held jobs on them. Every CPU figure of a sample (host, classes, sessions, jobs,
  machine quota) now divides by the time between the two scans, and a failed scan no longer
  shortens the next sample's interval.
- Queue order within a level: `slot serve --pass-for <seconds>` (300 by default). A queued job
  may be passed by later jobs of its own level only until it has waited this long; a job of a
  lower level never passes it, and 0 serves the queue by level, then arrival, as before. Inside
  that bound a job predicted to run 5 s or less (the 90th percentile of its last runs) starts
  ahead of longer jobs of its level, and a job that asks for more slots than are free no longer
  keeps the slots it collected from later jobs of its level that fit. Slots lent to an urgent job
  out of the headroom stay with urgent jobs. The job record has `predicted_ms`, `collected`,
  `ahead` and `queue_class`; `slot status` ends a queued line with `holds N of M` and `moved
  ahead` and its pool line with `N collected by queued jobs`, and both it and the web queue list
  the queue in the order the pool serves it. Why: jobs under 5 s carried half of the queue wait,
  and a job that asked for every slot stalled every job behind it.
- Eviction starts where the disk hold starts: its mark is the larger of `--evict-below` and
  `--min-free-disk` (`--evict-below 0` still turns it off), and a job that starts to wait for disk
  makes the daemon look at once instead of at the next minute. `slot status` prints the mark in
  force and says when the disk hold raised it. Why: with `--min-free-disk 50` and `--evict-below
  40`, up to 38 jobs stood queued for disk for 12 minutes and nothing was deleted.
- The CPU limit for starts (`slot cpu-limit`) counts the CPU of the pool's jobs and of the compile
  cache servers (an `sccache` whose parent is the service manager, and its compilers), not work
  outside both. A held job's reason names the parts (`host busy: pool 12.0 + compile cache 6.0 of
  20 CPUs, mark 17.0`), `slot cpu-limit` prints them and the CPUs outside both, `GET
  /api/cpu-limit` answers `pool`, `compile_cache` and `outside`, and `slot status` reads `starts
  held while the pool and the compile cache use more than N CPUs and M jobs run`. Why: work
  outside the pool held 278 jobs in the first hour the limit was on.
- A job runs with the open-file limit of the process that called `slot`, capped at the daemon's
  hard limit with one `slot: warning:` line. When it differs from the daemon's, the daemon starts
  the job through its own binary (`slot __exec`, hidden), which sets the limit and then executes
  the job's command in its place; the daemon's own limit never changes. Remote jobs keep the build
  server's limit; the stack size and other limits are not passed. Before, a daemon under systemd
  gave every job 1,024.
- A reload no longer drops output lines still queued for a client: the writer hands them on with
  its unsent bytes (a run of 14 to 40 lines went missing in 1 of 6 reloads at a load of about
  100). After a reload the new image puts every running job back in its table before it watches
  any, so a job that ended during the handover no longer makes the daemon add the slots the other
  jobs hold to the pool (+1, +6 and +18 seen).
- The daemon tests wait on the state they assert, not fixed times; the slowest handover test went
  from 44.5 s to 29.4 s at a load of 105 to 134.
- `rust-toolchain.toml` pins rustc 1.99.1 with rustfmt and clippy; the local gate and the gate,
  release and documentation workflows compile with it, and `task check` prints `rustc --version`
  first.
- A pull request whose changed files are all planning or prose files (`.engineering/`, `*.md`
  outside `website/` and `ui/`) passes `gate` and `Build documentation` without building; one that
  changes only `website/` runs the docs checks without the code's build and tests.
  `.github/scripts/pr-scope.sh` decides and prints the paths it saw.
- UX gates: the docs gates load each page once per theme and measure both gating sizes in that
  load (24 loads in place of 48, a third less CPU); both suites serve the built files with
  `ui/ux/serve.ts` in place of `vite preview` and `docusaurus serve`, and the docs gates take a
  free port, so two worktrees run them at once.
- Admission by share, off by default: `slot admission shares|slots [--floor N] [--max N]
  [--pressure P] [--min-budget CPUS]`, `GET` and `PUT /api/admission`, kept in `limits.json`
  beside the headroom; `slots` changes nothing. By share a job asks for the CPUs its earlier runs
  of the same repository and command used (a high percentile plus 15 %, at least 0.1, 1.0 with no
  history) and starts while the CPUs the pool and the compile cache servers used over the last
  5 s, the shares of jobs started in those 5 s and its own share fit a budget. The budget starts at
  three quarters of the host's CPUs, never passes the pool's CPU quota and never falls under its
  lower bound (half the CPUs by default), rises while finished jobs run near their best earlier
  wall time and falls when they run over twice it. At least `--floor` jobs (4) and at most
  `--max` (48) run; over the CPU pressure mark only short jobs start. The jobserver FIFO follows
  the budget; a job holds no slot of its own, and `--tokens N` is read as a share of N, capped at
  the budget. A job that uses almost no CPU lends its share to the queue while it idles; `Job`
  has `lent_s` and `admission`, `slot usage` charges the share less what was lent, and `slot
  status` and the web view show each job's asked and used share, the free share and the budget.
  The saved setting applies before the daemon takes or resumes any job, and until the first CPU
  figure every running job counts with its whole share. Back by slots, a job that started by
  share keeps one slot held back until it ends. Not built: a CPU weight per job, which needs a
  cgroup per job and so root. The budget rule is to be measured with shares on, on one host,
  before it is switched on anywhere else (design section 26).
- An unknown `/api/` path answers 404 for every method, so a newer client tells an older daemon
  from a failed call.

## 0.24.0 — 2026-11-22

- A CPU limit for starts: `slot cpu-limit <CPUs>` holds the start of a queued job while the whole
  host is that many CPUs busy or more and at least `--floor` jobs run (4 until one is named).
  `off` is the default and changes nothing. Why: on a host of 20 CPUs with a pool of 50 slots the
  host was 16.7 to 18.5 CPUs busy for three hours with an empty queue and 35 slots free, so slots
  no longer bounded what started, and about half of the compiler CPU ran in the compile cache
  server, outside the pool's CPU quota. The figure is the CPU time of every process on the host
  as the daemon already samples it once a second, as a mean over 10 seconds; a job that started in
  the last 30 seconds counts with the CPUs its earlier runs used (1 without history) until the
  figure shows it, so a dip under the mark starts only as many jobs as fit, in the queue's order.
  A held job holds no slot and says why (`host busy: 18.4 of 20 CPUs, mark 17.0`) in its client
  and in `slot status`; after 10 minutes it starts whatever the figure says. A mark above the
  host's CPU count never holds, and a mark under 1 is refused. What it costs: with the limit on a
  queue forms while the host is saturated, also when slots are free. The setting reaches
  queued jobs within 2 seconds, is kept in `limits.json` beside the headroom and holds across reloads;
  `GET` and `PUT /api/cpu-limit` read and change it (`{"cpu_busy_max": 17, "cpu_busy_floor": 4}`,
  `null` for off); `slot status` shows `starts held while the host is busier than 17.0 CPUs and 4
  jobs run` while it is on, `/api/state` has `cpu_busy_max` and `cpu_busy_floor`, and the census
  log gets a `cpu_hold` line at a job's first hold and a `cpu_hold_bound` line when the 10 minutes
  ran out.

## 0.23.0 — 2026-11-22

- A limit on build processes per job: `slot job-parallel <N>` makes the daemon start every job
  without the pool's jobserver and with `CARGO_BUILD_JOBS=<P>` and `MAKEFLAGS=-j<P>` instead, where
  P is N, or the slots the job holds when those are more (`slot --tokens`). `cargo` then makes a
  jobserver of its own with P tokens and `make` runs P recipes at once; the job still waits for
  and holds its slots, and only its compilers stop reading the pool's FIFO. `off` is the default
  and changes nothing. Why: with the pool's jobserver a freed slot goes to whoever reads the FIFO
  first, and the `cargo` processes of running jobs read it against the one daemon (on a pool of 50
  slots, 7 `cargo` processes against the daemon, one with 19 compilers, 10 jobs running and 49
  waiting); cargo ignores `CARGO_BUILD_JOBS` and `-j` while it inherits a jobserver, so a bound
  per job exists only outside the pool. What it costs: the pool then bounds jobs, not compilers,
  so S slots can run up to S × N build processes; a job no longer narrows when the pool is full;
  the bound is per `cargo` and per `make`, so a script that runs two at once runs twice as many.
  A `cargo` that is killed takes no slots of the pool with it. The setting applies to the jobs
  that start after the change, is kept in `limits.json` beside the headroom and holds across
  reloads; `GET` and `PUT /api/job-parallel` read and change it (`{"job_parallel": 4}`, `null`
  for off, under 1 refused); `slot status` shows `at most 4 build processes per job, outside the
  pool's slots` while it is on, and `/api/state` has `job_parallel`.

## 0.22.0 — 2026-11-22

- A run-time limit for jobs: `slot run-limit <duration>` makes the daemon end a job that has run
  longer (`10m`, `90s`, `1h` or plain seconds; `off` for no limit, which stays the default). The
  time counts from the job's start, not from when it queued. The job ends as with `slot cancel`
  (SIGTERM to its process group, SIGKILL 5 s later); the client exits 124 after one line with the
  job, its run time, its limit and how to name a longer one, and the job's record has
  `"timed_out":true` and `limit_s`. A job names its own time with `slot --timeout <duration>`,
  `slot run --timeout` or `SLOT_TIMEOUT`, up to the most a job may name (`slot run-limit --max`,
  1 h by default; a longer value is capped and the client is told). A `timeout` command in front
  of the job's command extends nothing: `slot timeout 1800 cargo test` ends at the limit, and
  when such a `timeout` names more than the job's limit the client prints one line at submission
  with the `slot --timeout` to use instead. The limit a job starts with stays its limit, across a
  reload too. The settings are kept in `limits.json`
  beside the headroom; `GET` and `PUT /api/run-limit` read and change them; `slot status` shows
  the limit when one is set.

## 0.21.1 — 2026-11-22

- Lost build slots come back while other processes have the pool open. A process killed with
  jobserver tokens in hand (a cancel, a `timeout`, a watcher that kills by run time) took them
  with it, and the daemon wrote them back only while no process but itself had the pool's FIFO
  open, which a busy pool never reaches: tokens stayed lost for up to 173 minutes, and after 12
  jobs were killed at once at least 19 to 23 of 50 slots were held by no live process while 30
  or more jobs waited. Every 15 s the daemon now counts the most each live process with the FIFO
  open can hold (for `cargo`, `make` and `ninja` its child processes less one, for any other
  process the larger of its child processes and threads less one, and one more for a process
  with several threads) and writes back what is missing beyond that, the smaller of two counts
  in a row: within 45 s. What a live process can hold by that count is never written back,
  however long the process holds it, and whether its children are compilers or clients of a
  compile server.
- More tokens in circulation than the pool has (a slot released twice) are taken out within
  45 s while jobs run; before, only when no job ran. Journal line, census event
  `tokens_removed`.
- `slot status` shows `lost slots: <n> that no job and no live process holds, oldest for <time>`
  between the two counts; `/api/state` carries `tokens_unaccounted` and
  `tokens_unaccounted_since_ms`, `/metrics` the gauge `slot_tokens_unaccounted`. The census
  event `tokens_restored` also names `holders` and `bound`.

## 0.21.0 — 2026-11-22

- The delay after which a waiting `normal` or `low` job counts as `high` changes while the daemon
  runs: `slot priority promote-after <seconds>` (0 never raises a job, `default` returns to
  `slot serve --promote-after`). It applies to the jobs already queued, is kept in
  `priority.json` beside the rules and wins over the option after a reload or a restart.
  `PUT /api/priority` takes `{"promote_after_s": 300}` (`null` for the option's value) beside the
  rule form; `GET /api/priority` and `slot priority list` show the delay in force.
  The option alone took effect only at a full restart, because a reload keeps the daemon's
  arguments.
- A cargo build no longer takes a slot while another job of the pool compiles in its target
  directory, where cargo would only print "Blocking waiting for file lock on build directory"
  and hold the slot idle. It stays queued with the reason `target directory in use by job N`
  (`slot status`, the web view, one line from the client) and the jobs behind it start. It
  goes on when that job has compiled or ended; several builds of one directory start by
  level, then age. A job that waited outside the queue for slots (for its target directory,
  for memory or disk) comes back to the place its age gives it, not to the end.
- Web view: a fourth line chart, **Compile cache**, shows per minute the share of compiler calls
  sccache answered from its cache, of all calls and of the cacheable ones; the daemon serves the
  calls per minute of the last six hours as `GET /api/cache-history`.

## 0.20.0 — 2026-11-22

- Web view: one row of three line charts stands directly under the header (cores used on the
  whole machine, pressure, pool size), and the job tables read Running, Queue, Finished.

- Web view: the job tables come above the host panel, the Sessions table and the timeline. The header counts the jobs that failed in
  the last 60 minutes, and the count opens the output of the newest one. Opening an output by
  keyboard moves the focus into the pane; Escape closes it and returns the focus. A unit row
  opens only by its toggle. The Finished table states a result in words ("failed (101)", "ok",
  "cancelled"), and a build outside the pool is "not on slot" everywhere.

- Web view: the header is 80 px high in every state at 1280 px and wider; a refused change, the
  machine line, a pending upgrade and a call share its second line instead of adding rows. A "?"
  button explains each header item, by pointer or keyboard. The wait percentiles, the cores of
  the jobs, the host CPU, the pressure and the recommended pool size moved from the header to the
  section each belongs to, with their current values.

- Web view: the Queue, Running and Finished tables have one column order and one set of widths,
  so slots, wait, time and cores stand in line down the page. Finished has a result mark and the
  slots a job held. A job's session is named by its title. A queued job that waits for disk space
  or memory says so in a line under its row, and the Queue gives the command the rest of the row.

- Web view: the output pane says the job's state in words ("queued, #4 of 6, waiting 31s",
  "running, output live", "failed (exit 101)") and lists what the tables cut or leave out: the
  whole command, the directory, the session with its id, slots, times, cores and memory.

- Web view: the page title carries the counts ("6 running, 2 queued · slot <host>"), and the
  page has an icon. Text is 14 px, commands and output 13 px, nothing below 12 px. Nothing
  blinks: a running bar of the timeline has a steady edge, and red is left to a failure.

- Web view: the Sessions table shows the id in every row and "no title" for a session without
  one; a timeline bar ends a cut label with "…".

- Once a minute the daemon appends the counters of each running sccache server to
  `<state-dir>/cache.jsonl`: Rust and C/C++ hits and misses, the calls the server refused by
  reason, the cache size and its limit, and `restarted` when a counter went down. sccache keeps
  these counters only since its last start. The daemon reads them only from a server that is
  running, so it never starts one, and not from a server that used no CPU time since its last
  line, so the reading does not keep an idle server from its idle exit.
  `slot serve --sccache-watch false` turns it off.

- The record of a rust job in the job log has `cache_env`: the values of `RUSTC_WRAPPER`,
  `CARGO_BUILD_RUSTC_WRAPPER`, `CARGO_INCREMENTAL`, `RUSTFLAGS`, `CARGO_ENCODED_RUSTFLAGS`,
  `SCCACHE_SERVER_PORT`, `SCCACHE_DIR`, `RUSTUP_TOOLCHAIN` and every `CARGO_PROFILE_*` the job
  ran with, and no other variable.

- A rust job whose environment switches the compile cache off (`RUSTC_WRAPPER` empty) or uses
  an sccache server of its own (`SCCACHE_SERVER_PORT` other than 4226, or `SCCACHE_DIR`) is
  warned: the client prints `slot: warning: …` when the job starts, `slot status` lists the
  warning under the job, and the job's record has it in `warnings`. The job runs unchanged.

## 0.19.1 — 2026-11-22

- Jobs show their Git repository and linked worktree name. Directories outside
  Git show **No repository**. Historical jobs keep their recorded names marked
  **Unverified**, or show **Repository unknown** when the name is empty.

- Daemon handovers preserve recent chart samples and job output when the sending daemon supports it.

- The CPU and queue chart uses the saved ten-second census for up to six hours of history after a reload.

- The Claude Code hook refuses a build or test command behind shell syntax that hid it before:
  behind `if`, `then`, `elif`, `else`, `while`, `until`, `do`, `!` or `{`
  (`for p in a b; do task check; done`), behind `timeout -k 5`, `-s KILL` or `--signal KILL`
  and `env -u FOO` or `-C dir`, inside a shell flag cluster that ends in `c` (`bash -euc '…'`,
  `sh -xc '…'`) and inside `eval '…'`. The `slot` command in its advice has the words as they
  were written: quotes stay (`slot pytest -k "a and b" tests`) and a trailing number is no
  longer dropped (`slot make -j 8`, `--workers 2`); a file-descriptor digit before `>` or `<`
  (`2>&1`) is still left out.

## 0.19.0 — 2026-11-19

- Eviction of local target directories deletes by default: `--evict-dry-run` defaults to false.
  `<state-dir>/evict.json` (`{"dry_run": true}`) turns the dry run on or off without changing the
  service's command line; the daemon reads it every minute, no reload needed, and it wins over
  the flag. A dry run under the floor says so plainly in the journal and in `slot status`:
  `eviction is off (dry run), would free <n> GB in <k> dir(s)`.

## 0.18.0 — 2026-11-15

- `slot gate --dir <dir>`: builds in a sandbox that cannot reach the daemon's socket (no
  `AF_UNIX`, no network) queue in the pool through FIFOs in a directory shared with the host, and
  run in the sandbox. The client sets `SLOT_GATE=<dir>`; the daemon starts only `slot hold`, which
  keeps the job's slots until the sandboxed command exits. The gate hard-links the pool's FIFO
  into the directory, so cargo, rustc and make in the sandbox draw from the same jobserver.
  `contrib/slot-gate.service` runs it. The request carries `hold_for`, defaulting, so older
  daemons and clients keep working. Design: `docs/design.md` § 19.
- The lost-token watch counts a process as holding the pool FIFO by device and inode, so a
  sandboxed compiler holding it through the gate's link is seen.
- `contrib/slot-shim` sends `make` through slot when linked as `make`.

## 0.17.0 — 2026-11-13

- `slot usage` and `GET /api/usage`: slot time, CPU time, jobs and median queue wait per project,
  repository or session over a window, from the job log; the web UI shows the same as **Share of
  the pool**. Slot time and CPU time are both shown because jobs that hold slots while idle make
  them differ.
- Every job records its `project`: the namespace of its repository's git remote, or `project =`
  in `.slot.toml`. Several repositories of one project and all their worktrees share it.
- Job priority. Every job has a queue level, `urgent`, `high`, `normal` (the default) or `low`:
  `slot --priority <level> <command…>`, `slot run --priority <level>` or `SLOT_PRIORITY`, the
  flag winning over the variable. Queued `urgent` jobs get free slots first, then `high`,
  `normal` and `low`, first in first out inside a level; a running job is never stopped. An
  `urgent` job, for repair work on the pool, starts with no slot free on one the headroom holds
  back, at most `pool − limit` of them at a time, and they go back to the held-back set when
  it ends.
  `slot priority set <pattern> <level>`, `unset <pattern>` and `list` keep a rule table per
  repository (exact name or a glob with `*`) that wins over the job's own level, changes
  without a restart, re-ranks the queue at once and is saved in `priority.json` beside
  `limits.json`; `GET` and `PUT /api/priority` read and change it. The table starts empty.
  A job queued more than 30 minutes (`slot serve --promote-after`, seconds, 0 off) counts as
  `high`, ahead of every `high` job that arrived after it, so a stream of `high` jobs cannot
  starve a `low` one. The head of the queue still gathers slots until it has all it asked for,
  so a multi-slot job is not overtaken by later 1-slot jobs of its level or below; a later
  higher job overtakes it and takes the slots it gathered. `slot status` ends each queued line
  with the level and lists the queue in the order it is served; the web UI's queue has a
  `level` column and the same order. The request carries `priority` and jobs carry `priority`
  and `promoted`, all defaulting, so older clients and daemons keep working.

## 0.16.0 — 2026-11-13

- `slot --tokens N <command…>` and `slot run --tokens N <command…>`, or `SLOT_TOKENS=N`: the job
  holds N slots in place of its toolchain's count. A script that starts `docker run --cpus 4`
  held one slot while the container, outside the daemon's cgroup, used 4 CPUs. The daemon caps
  N at the slots in circulation and says so, so the job never waits forever. Remote mode
  forwards it; `--local` ignores it. The request carries it in a new field `tokens`, which an
  older daemon ignores.
- Headroom of the machine. `slot headroom --base machine` (or `"headroom_base": "machine"` in
  `PUT /api/limits`) makes the headroom a share of the whole machine instead of the pool: each
  tick the daemon takes the CPUs busy on the host less its own cgroup's CPU as the load outside
  the pool, averages it over 30 s, and sets the pool's `CPUQuota` to the target less that load,
  between 2 CPUs and the target, at most every 10 s and only on a change of half a CPU. The base
  is saved in `limits.json` and changes without a restart; `pool`, the default, keeps the fixed
  quota. The pool's quota alone left a machine at 45 % headroom 93 to 99 % busy: a docker
  container a build started, compile servers outside the unit and the desktop ran outside it.
- `slot status` and the web header show, on the machine base, one line: CPUs busy, the target,
  the pool's quota, the load outside the pool and its five busiest owners (applications,
  login sessions, docker containers, `system.slice`), marked `over target` when the pool is at
  its floor and the machine is still busier. `slot headroom` names the base. State and ticks
  carry `headroom_base`, `busy_cores`, `target_cores`, `outside_cores`, `outside`,
  `quota_floor` and `over_target`; ticks also `cpu_quota_percent`.

## 0.15.0 — 2026-11-13

- Disk-budget eviction of local Cargo target directories. Every 60 s, when the filesystem of a
  target directory that jobs built into has less than `--evict-below` free (GB, default 40, 0
  off), the daemon picks the least recently built target directories until the filesystem would
  have 10 GB more than that. It keeps a directory whose repository has a queued or running job
  or that was built in the last 2 h, and deletes only a directory Cargo tagged with
  `CACHEDIR.TAG` that a logged job built into. A target directory of a managed worktree goes
  through `worktree discard-cache`. `--evict-dry-run` is on by default: the journal and
  `slot status` say what would be deleted and nothing is. Jobs in the log carry `target_dir`.
- A second `systemctl --user reload slot` while a handover runs no longer ends the daemon: it
  is ignored while the new image starts, caught from then on, and runs as one more handover.

## 0.14.0 — 2026-11-12

- The web view shows a pending upgrade in its header, the line `slot status` prints
  (`upgrade pending: no new job starts for 42s, …`). While a reload holds admission, builds
  queue with slots free, and the page did not say why. Every tick carries the line in a new
  field `upgrade`; a page reading an older daemon keeps the line from `/api/state`.
- Below 1100 px the output pane is a sheet along the bottom of the window, 45 % high, and the
  job tables keep the full width. At 960 px the Running, Finished and Sessions tables ran 135,
  90 and 70 px under the pane. Opening an output there scrolls the job's row above the sheet.
- The UX gates serve the built UI on a free port instead of 4317, so two worktrees run
  `task check` at the same time; `UX_PORT` sets the port.

## 0.13.0 — 2026-11-12

- `ignite` toolchain: `ignite test` and `ignite conformance` hold one slot per test job (`--jobs`,
  else 4) and get `IGNITE_TEST_JOBS` set to the slots held (read by ignite 0.16.3 and later);
  `ignite build` and other subcommands hold one slot; the jobserver is passed down so the Rust
  target's cargo shares the pool. Before, a `slot ignite test` held one slot while its runner
  started one test job per core.
- `contrib/slot-shim` also covers `ignite build|test|conformance`; link it as `ignite` next to
  `cargo` and `go`. The LLVM target's `clang` reads no jobserver and had run outside the pool.
- The Claude Code hook also refuses, outside the pool: `tsc`, `vue-tsc`, `eslint`; `vite`,
  `docusaurus`, `webpack`, `next`, `nuxt`, `astro`, `rollup`, `esbuild` with `build`;
  package.json scripts named like a build (`pnpm build`, `yarn typecheck`, `npm run test:unit`);
  `node --test` and `node <script> gate|test|check|…`; `zig build|test|build-exe|build-lib|build-obj`
  (also `zig.sh`); `pytest` and `python -m pytest`; maven and gradle goals that compile or test;
  `ignite build|test|conformance` without the shim. Watch, serve and print-only flags pass.
- The host census counts `ignite`, `zig-compile`, `python-test` and `jvm-build`, so CPU of
  those tools outside the pool shows in the census and `slot sessions`.
- Measured coverage of every toolchain seen on one workstation, and why docker build, helm and
  valgrind stay outside: `docs/design.md` § 17, and the agent integration page.

## 0.12.0 — 2026-11-12

- Lost build slots come back while the pool is busy. A compiler killed while it held jobserver
  tokens (a cancel, a `timeout`) took them with it, and the daemon gave them back only when no job
  ran at all, which a busy machine never reached: on 2026-11-12 8 of 12 slots were gone and 19
  jobs queued while the host used 3 to 4 cores. Every 15 s while jobs wait and the FIFO is empty,
  if no other process has the FIFO open, the daemon writes the shortfall back (the smaller of two
  passes); journal line, census event `tokens_restored`, `/metrics`
  `slot_tokens_restored_total`.
- Call mode no longer counts Claude Code's voice input as a call when Claude Code runs from a
  per-version install (`.../claude/versions/<version>`, process name = the version): the ignore
  list now also matches the names in the executable's path, for PipeWire clients and camera
  holders.

## 0.11.0 — 2026-11-12

- `systemctl --user reload slot` re-executes at once and hands every running and queued job to
  the new binary: processes, slots, output pipes, client connections and unsent output carry
  over, so a long job no longer delays an upgrade and no job waits for one. Clients see no
  change; queued jobs keep their id and place. The journal says `slot: handing N running and M
  queued job(s) …` and `slot: took over …`. A new binary older than 0.11.0, or
  `slot serve --handover false`, gets the bounded hold of 0.5.1. Upgrading from an older daemon
  uses its own rule once.
- The jobserver reader and the output pumps poll, so they stop between two reads; a client's
  frames are written as its socket takes them instead of one write at a time.

## 0.10.0 — 2026-11-12

- `slot shell`: an ssh forced command for a build server. With
  `command="/path/to/slot shell",restrict` on the key in `authorized_keys`, the key runs only
  the steps of remote mode: the sync into the workspace root, a fetch from the workspace root or
  the cache root's `target/`, `slot proxy` and `slot cancel <id>`. Anything else, rsync options
  that name another file or program, `..` in paths and shell operators exit 126 with
  `slot shell: refused: …`. Tested against a KVM guest. It narrows the key; it is not a
  sandbox, since a job is any command.

## 0.9.0 — 2026-11-12

- Build servers seed a new worktree's target directory from the most recently used one of the
  same repository by reflink, so dependencies are not compiled again; on a filesystem without
  reflinks (ext4) nothing is copied. Tested on xfs: the second worktree of ess built in 1 s
  after a 0.4 s reflink, against 140 s cold.
- `slot serve --cache-budget GB`: before a remote job, target directories and their source
  copies are deleted, least recently used first, while together they take more than the budget
  or the disk is under `--min-free-disk`. A worktree with a queued or running job is kept.
  Every deletion and seed is printed on the job's stderr and in the journal.

## 0.8.0 — 2026-11-12

- `slot --fetch PATH <command…>`: with `SLOT_HOST` set, copies PATH back from the build server
  after the job exits 0. PATH is relative to the repository root; `target/…` is read from the
  job's target directory on the server, which the daemon now reports in the job's `start` frame.
  Repeatable; a failed copy makes the exit code 1.
- A remote job that does not start (host down, ssh refused, sync failed) now goes to this
  machine's daemon and waits in the local pool. Before, the command ran directly, outside the
  pool: a full disk on a test server sent a 100-crate build to the workstation unpooled.

## 0.7.0 — 2026-11-12

- Remote mode no longer sends credentials to the build server. The rust toolchain admitted every
  `CARGO_*` variable, so `CARGO_REGISTRY_TOKEN` and `CARGO_REGISTRIES_<name>_TOKEN` reached the
  job on the server. A name that contains `TOKEN`, `SECRET`, `PASSWORD`, `PASSWD`, `CREDENTIAL`,
  `_KEY` or `AUTH` now crosses only when `SLOT_ENV` names it.
- `.slot.toml` at the repository root: `local = ["cargo test -p integration", …]` lists command
  prefixes that run on this machine's daemon while `SLOT_HOST` sends every other build to the
  server, for tests that need docker, a database or a local port. slot prints one line when it
  applies, and reports and ignores a file that does not parse.
- Remote mode measured against a real server for the first time (a KVM guest, 4 vCPUs): sync,
  build, streaming, exit codes, cancel and a dropped connection, in `docs/design.md` § 16. The
  remote-builds page now covers the disk floor on a small server and `slot` missing from `PATH`
  over ssh.

## 0.6.0 — 2026-11-12

- Web UI: buttons 0 %, 20 % and 40 % beside the headroom slider set the headroom in one click.
- Web UI: a refused headroom change (a daemon older than the page) puts the slider back on the
  daemon's value; before, the caption kept showing the refused value as applied.
- Web UI: the job number in the Queue, Running and Finished tables is a button that opens the
  job's output, so finished and queued jobs open by keyboard too; the output text takes focus and
  scrolls by keyboard; small buttons are at least 24 px.
- Web UI: no sideways scroll at 1280 px, and the job tables no longer run under the output pane:
  the command column takes the width that is left, long repository names are cut with the full
  name in the hover title, below 1150 px the job panel drops the sparkline, memory and session
  columns (the Sessions table its memory and sparkline), and below 900 px the Running table drops
  its live-output button (the job number opens the same output).
- Web UI: the panels appear once the jobs and the history have loaded, so the page no longer
  jumps while it loads.
- `task check` runs UX gates on the web UI (`ui/ux`): layout, axe-core accessibility and a
  screen-only user that completes every scenario task by pointer and by keyboard, at 1366x768
  and 1280x720, in DejaVu Sans on every host, with a ratchet on the measured values. The docs hero screenshots are rendered
  from the same scenarios (`task ui-hero`).

## 0.5.2 — 2026-11-12

- A client that gets SIGINT, SIGTERM or SIGHUP cancels its job and exits with 128 + the signal;
  it no longer sends the job again. Closing the connection on the signal looked to the client
  like a daemon upgrade, so a queued job was sent a second time, which the job log kept as a
  second job (18 of 60 queued jobs under `timeout` in a test against 0.5.1). A job is still
  sent again when the daemon closed the connection before the job started and no signal came.

## 0.5.1 — 2026-11-12

- Upgrades no longer stop every build behind one long job. After `systemctl --user reload
  slot`, admission is held for at most 60 s (`--upgrade-hold`, 0 never holds); if a job still
  runs, jobs start again and the next hold begins when the running jobs are expected to end
  within 60 s (from earlier runs of the same command in the same repository) or none runs. The
  daemon re-executes at the first moment no job runs. `slot status` shows `upgrade pending: …`
  until then. The design document describes a handover re-exec that keeps running jobs (not
  built).
- `slot cancel <id>` cancels a queued or running job: a running job's process group gets
  SIGTERM, then SIGKILL after 5 s, as when its client goes away. Only the user who submitted
  the job may cancel it. Before, `slot cancel 12` was queued as a build running a command named
  `cancel`.
- A command not found on `PATH` is refused at once (`slot: cancle: not found in PATH; not
  queued`, exit 127) instead of waiting in the queue and failing when it starts.

## 0.5.0 — 2026-11-12

- Call mode: while a call uses the microphone or the camera, the headroom goes up to the call
  value (40 % by default) and back to the earlier value 2 minutes after the call. A call is a
  running PipeWire capture stream linked to a microphone or camera node, or a process holding
  `/dev/video*`; level meters, mixers and Claude Code's voice input (process `claude`) are
  ignored (`--call-ignore`), and an ALSA client is named by its process. `slot headroom --call
  N` sets the call value at run time (0 turns call mode off); a headroom set by hand during a
  call stays. `slot headroom`, `slot status`, `/api/limits` and a badge in the web UI show it.
- sccache watchdog: the daemon watches the private jobserver of every sccache server of its
  user and writes back the tokens a server lost, once for 60 s it had none free, fewer compiles
  than its pool, a waiting client and under 5 % CPU. Each refill goes to the journal and
  `census.jsonl`; `slot status` shows the count. `--sccache-watch false` turns it off.
- Admission by disk and memory: a job waits before taking slots while the filesystem of its
  directory or `CARGO_TARGET_DIR` has under 25 GB free (`--min-free-disk`), or while available
  memory does not cover its expected peak (90th percentile of earlier runs of the same command)
  plus 4 GB (`--min-free-memory`). A job is never held for memory while no other job runs. The
  client prints `slot: job N waiting: …`; `slot status` and the web UI queue show the reason
  (`wait_reason` on the job).
- Jobs under `--memory-max` run in `slot.slice`, and the headroom's CPU quota is set there too;
  before, they ran outside the quota.
- The design document records the measurement behind not coalescing identical concurrent jobs:
  0 overlapping pairs from different sessions in 740 jobs.

## 0.4.1 — 2026-11-12

- The pool no longer shrinks when the daemon re-executes under a headroom. The default
  `--tokens` and the core count behind the CPU quota came from
  `std::thread::available_parallelism`, which reads the cgroup's CPU quota: a daemon that
  re-executed inside its own `CPUQuota=1200%` on 20 cores counted 12, sized the pool at 12 and
  set the next quota from 12 (measured: `headroom 40%: 7 of 12 tokens, cpu quota 720%`). They now
  count the CPUs online on the host (`/sys/devices/system/cpu/online`), and so do `node-test`
  percentage worker flags.

## 0.4.0 — 2026-11-12

- The repository is a Claude Code plugin marketplace (`claude plugin marketplace add
  beyond10x/slot`, then `claude plugin install slot@slot`). The plugin in `plugin/` carries the
  skills `slot:building`, `slot:init`, `slot:upgrade` and `/slot:warden`, a `PreToolUse` hook and
  the `slot-warden` agent. The skills also load in Codex.
- `slot hook claude-pre-tool-use`: the hook. It refuses a Bash command that compiles or runs
  tests outside the pool (cargo and go builds unless the PATH shim is installed, `make` and `task`
  targets named like a build, `npm test`, vitest, jest, `playwright test`), with one line naming the `slot` command to run
  instead. `SLOT_HOOK=warn` adds the line as context instead; `SLOT_HOOK=off` allows everything.
- `slot sessions`: agent sessions that build outside the pool, with cores averaged over the
  daemon's history and the commands running outside it now; `--json` for agents.
- Commands named `headroom`, `sessions` or `hook` need `slot run`.

## 0.3.1 — 2026-11-12

- Upgrades finish on a busy machine. After `systemctl --user reload slot` the daemon admits no
  new job and re-executes once no job runs, instead of waiting for a moment with nothing queued.
  A client whose job was still queued sees the connection close before the new `start` frame,
  prints `slot: daemon upgrading, resubmitting job`, waits up to 10 minutes for the socket and
  sends the same command again, once. A job that had started is not resubmitted.
- A 0.3.0 client could not read `/api/state` from an older daemon (`missing field limit`). Every
  field added since 0.1.0 now has a default.
- `slot headroom` and the web UI's slider report an older daemon instead of treating its web
  page as success: an answer that is not JSON is an error, and the daemon now answers an unknown
  `/api/` path with 404.
- A job that asked for more slots than circulate under a headroom released the slots it asked
  for instead of the ones it held, adding tokens to the pool until the next idle moment.

## 0.3.0 — 2026-11-12

- Headroom: a share of the CPU, 0 to 50 %, that builds leave free for interactive work such as a
  video call. Set it with the **headroom for you** slider in the web UI, `slot headroom <percent>`
  or `PUT /api/limits`. The daemon withholds that share of the pool's tokens and, when it runs as a
  systemd user unit, sets `CPUQuota` on its own unit, which caps every job together. The value is
  saved in `limits.json` and applied again after a restart. `slot serve --cpu-quota false` keeps
  the token limit only.
- `/api/state` and the event stream report `limit` (tokens that circulate) and
  `headroom_percent`; `slot status` shows `X of Y slots (H% headroom)` while a headroom is set.
- `PUT /api/limits` needs `Content-Type: application/json` and refuses a browser `Origin` other
  than the daemon's own page.

## 0.2.0 — 2026-11-12

- `contrib/slot-shim`: a `PATH` shim for `cargo` and `go` that sends build, check, test, clippy,
  doc, bench and install through slot and runs every other subcommand directly, so agent shells
  build through the pool without a `slot` prefix. Install steps in the agent-integration guide.
- Test binaries no longer start one thread per core while their job holds one slot:
  `RUST_TEST_THREADS` and `NEXTEST_TEST_THREADS` default to `slot serve --test-threads` (a
  quarter of the pool, at least 2) in rust jobs and in wrapper jobs such as `slot task check` or
  `slot bash script.sh`; `GOMAXPROCS` defaults to the slots held in go jobs and to
  `--test-threads` in wrapper jobs. A value the caller set is kept.
- New toolchain `node-test` for `playwright test`, vitest and jest: the job holds one slot per
  worker, read from `--workers`/`-j` or `--maxWorkers`/`--max-workers`, 4 without a flag.
- `slot status` prints the pool and every queued and running job. A command named `status` now
  needs `slot run status`.

## 0.1.0 — 2026-11-12

- `slot serve`: one GNU-make jobserver FIFO shared by every job, FIFO-ordered admission,
  lost-token reconcile, job log (`jobs.jsonl`), live web UI with timeline, slot usage, cores per
  toolchain and job output.
- `slot <command…>` and `slot run`: run a build through the daemon; local fallback when the
  daemon is unreachable. A queued job prints its place in the queue. `slot` inside a slot job
  runs the command directly instead of queueing a second time.
- Remote mode (`SLOT_HOST`): rsync over ssh, `slot proxy`, server-side target directories,
  output paths rewritten to the client.
- Toolchains: rust (jobserver client), go (fixed slots, `-p` follows), other (one slot, pool
  passed down).
- CPU attribution of work done inside sccache through the `SLOT_JOB` tag.
- Live view of each running job: the crates, build scripts, tests and linkers it runs now.
- Builds run at CPU weight 20 (`--cpu-weight`), so interactive work keeps the CPU.
- Host census: every build process on the host, run by slot or not, by tool family and by agent
  session, with CPU, memory and IO pressure; written to `census.jsonl` every 10 s and shown in
  the UI. `GET /api/history` serves the last 15 minutes.
- Adaptive pool controller in observe mode: it computes a recommended pool size from cores,
  builds slot does not run and CPU pressure, and does not enforce it yet.
- `GET /metrics` in Prometheus text format.
- SIGTERM drains running jobs; SIGHUP re-executes a new binary in place once no job is active.
  `contrib/slot.service` is a systemd user unit that uses both.
