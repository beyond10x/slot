# Remote pool: ephemeral build hosts in the slot pool — design

Status: proposal, 2026-11-23, against slot 0.30.0. Nothing here is built unless a file and line of
that tree is cited for it. `§ n` is a section of this document; sections of `docs/design.md` are
written `design.md § n`.

A citation `file:line` was checked against the tree on 2026-11-23. *(unverified)* marks a claim
taken from memory of a tool's or a provider's behaviour, or reasoned from code without a run;
phase 0 (§ 9.1) names the check for each one that matters. Prices are approximate public list
prices, remembered rather than looked up. Transfer times are arithmetic on assumed link speeds
(§ 4.6), not measurements.

Terms: the **workstation** is the shared machine; its daemon is the **local daemon**. A **host**
is a cloud build machine with its own **host daemon**. The **mirror** is a worktree's copy on a
host. A **placement** is the local daemon's decision that a worktree builds on a host.

## 1. Question and short answer

**Situation**, as given by the operator on 2026-11-23: one 20-CPU workstation shared by about 24
agent sessions; load average 60 to 110; CPU pressure 40 to 70 %; I/O pressure about 40 %; queue
wait p90 about 30 minutes; 50 to 60 GB of free disk, with target directories of 13 to 25 GB per
worktree.

**Question.** Can slot become a compute cluster for compiling: a bigger, ephemeral cloud host (one
night, or 4 hours) that only we can reach joins the pool, and jobs run there?

**Answer.** Yes. The data path exists and is measured against a second machine (design.md § 16):
remote mode, chosen per command with `SLOT_HOST`. What is missing is the local daemon choosing the
host, one view and one job log, and a host that starts, stops and cannot overspend. Phase 0
(§ 9.1) needs no code: one host started by hand, used by chosen commands, measured.

### 1.1 How does the code get there?

**slot pushes the working tree. The host never clones and needs no access to any repository.**

- Before every remote job the client runs `rsync` over ssh from the worktree's root to the mirror
  (`crates/slot/src/client.rs:615-634`). What goes: every file `.gitignore` does not exclude, so
  committed, modified and untracked files alike. What stays: gitignored files, `.git`, `target/`.
- Each worktree has its own mirror. The first push sends the whole tree (132 MB for the 66-crate
  workspace of design.md § 16: about 11 s at 100 Mbit/s up, 53 s at 20 Mbit/s). Every later push
  sends the changed files only: 0.29 s measured on one machine, estimated 0.5 to 1.5 s to a cloud.
- The host needs outbound HTTPS to the public crate registry, nothing else of ours.
- Four kinds of repository do not build this way today and are found by the pre-flight of § 9.1:
  a path dependency outside the repository, a private git dependency, a build input that is
  gitignored, a build script that calls `git` (§ 4.4).
- The push sends whatever lies in the tree. A secret in an untracked file that no `.gitignore`
  covers reaches the host (§ 8.4). Look at the file list once per repository before opting it in.

### 1.2 How does a result come back?

**Output and exit code always; a file only when named; the target directory never.**

- Standard output, standard error and the exit code stream back as the job runs. Paths in
  compiler messages are rewritten to the local checkout (`server/mod.rs:1627-1654`,
  `rewrite.rs:1-2`). For `check`, `clippy` and `test` that is the whole result.
- A binary to run locally is named with `--fetch` (built in 0.8.0):

  ```
  slot --fetch target/debug/mytool cargo build -p mytool
  ./target/debug/mytool
  ```

  After exit 0 the client copies that path from the host's target directory to the same path in
  the local checkout (`client.rs:194`, `:264-298`). A debug binary of 410 MB (design.md § 16)
  takes about 33 s at 100 Mbit/s down; a stripped release binary of 5 to 30 MB under 3 s (§ 5.2).
- Without `--fetch` the local `target/` is not touched. A binary already there is from the last
  local build and is stale. Nothing warns about that today (§ 5.1).
- A fetch runs only after exit 0, so the artifacts of a failed test run stay on the host (§ 5.3).
- Files a job writes into the tree (`Cargo.lock`, snapshots, formatted sources) stay in the mirror
  and are deleted by the next push. Commands that exist to write the tree stay local (§ 5.5).
- `cargo run` would run the program on the host. It stays local (§ 5.4).

### 1.3 What to do, in order

1. Phase 0 (§ 9.1): one on-demand host for 4 hours, started by hand, about $13. Two to four
   repositories that pass the pre-flight use it with `SLOT_HOST` in front of chosen commands. It
   yields the numbers this document only estimates.
2. Phase 1 (§ 9.2): the local daemon places opted-in worktrees on that one host, up to a fixed
   number, and records it. About a week of agent work, an estimate.
3. Phases 2 and 3: slot starts and stops hosts itself, then automatic start and warm caches.

## 2. What slot does today

### 2.1 The remote data path

| step | what happens | where |
|---|---|---|
| choose | `SLOT_HOST` in the caller's environment sends the command remote, unless `.slot.toml` lists it as `local` | `client.rs:137`, `:185-189`, `:303-318` |
| identify | workspace id = first 8 bytes of SHA-256 of `hostname:repo-root`, so each worktree has its own mirror | `client.rs:571-577` |
| siblings | before the push the client asks the host for mirrors of the same repository (normalized `origin` URL, `mirror.rs`) with `slot __siblings`; up to four go to rsync as `--copy-dest`, so a new mirror receives only what differs from them; a host without the step gets the plain push | `sync.rs`, `mirror.rs`, `shell.rs` |
| push | `rsync -a --delete --mkpath --filter=':- .gitignore' --exclude=/.git` plus the toolchain's excludes (`/target/` for rust, `/node_modules/` for node tests, none for go) to `<host>:<ws-root>/<id>/` | `sync.rs`, `toolchain.rs:222-224`, `:272-274`, `:375-377` |
| ssh | default `ssh -o ControlMaster=auto -o ControlPersist=10m -o ControlPath=… -o ConnectTimeout=3`; `SLOT_SSH` replaces it, split on whitespace | `client.rs:115-135` |
| environment | names the toolchain admits and that do not look like a credential, plus names in `SLOT_ENV` | `client.rs:635-639`, `toolchain.rs:163-176`, `:212-220` |
| run | `ssh <host> slot proxy` bridges to the host daemon's socket; the job starts from the host daemon's environment with the client's admitted names on top | `client.rs:645-662`, `server/mod.rs:1579-1590` |
| command | the host runs `argv[0]` as the client sent it; a name with a slash is not looked up | `server/mod.rs:2088-2111`, `:4816-4819` |
| caches | remote rust jobs get `CARGO_TARGET_DIR=<cache-root>/target/<id>`, go jobs `GOCACHE=<cache-root>/go-build` | `toolchain.rs:203-210`, `:255-257`, `server/mod.rs:2029-2044` |
| seed, evict | before a remote job queues, a new workspace's target directory is a reflink copy of the most recently used one of the same repository; least recently used ones are deleted over the budget or under the disk floor | `server/cache.rs:144-195`, `:201-205`, `server/mod.rs:1839-1841`, `:2337-2378` |
| output | streamed line by line; the mirror path is rewritten to the client's repository path | `rewrite.rs:1-2`, `server/mod.rs:1627-1654`, `:1475-1494` |
| fetch | after exit 0 only, each `--fetch PATH` is copied back with `rsync -a --mkpath`; `target/…` is read from the job's target directory, which the host reports in its `start` frame | `client.rs:194`, `:230-298`, `server/mod.rs:2143-2169` |
| measure | one stderr line per sync: bytes sent (rsync `--stats`) and seconds | `sync.rs` |
| fallback | when the sync fails, stalls (no byte read or written for `SLOT_SYNC_STALL`, 60 s) or runs past `SLOT_SYNC_LIMIT` (5 min), or ssh cannot be started, the job goes to the local daemon with one stderr line; a stalled rsync is killed with its ssh | `client.rs:200-207`, `sync.rs` |
| lost connection | `slot: connection to daemon lost`, exit 3, whether or not the job had started; the host cancels the job | `client.rs:190-192`, `:683-686`, design.md § 16 |
| access | forced command `slot shell` accepts only the sync, the fetch, `slot proxy`, `slot cancel` and `slot __siblings` | `shell.rs:1-40`, `:143-164`, `:207-225` |
| job log | the host daemon writes its own `jobs.jsonl`; a job carries `remote: true` | `server/mod.rs:640`, `:1766`, `types.rs:78-79` |

### 2.2 Measured limits

design.md § 16 ran remote mode against a 4-vCPU, 8 GB KVM guest on the same workstation, so every
number below is loopback, not a cloud link:

- 66-crate workspace, 9,199 tracked files, 132 MB synced: first sync 1.75 s; a sync with nothing
  or one file changed 0.29 s.
- Cold `cargo build` of its CLI on 4 slots: 160.7 s, 426 CPU-seconds, peak RSS 3.6 GB, target
  directory 4.0 GB. No-op rebuild 0.61 s; one line changed 4.5 s.
- The same build on the workstation's busy pool: 124 s in the queue, then 308 s.
- `--fetch target/debug/<cli>` brought back 410 MB.
- A second worktree got 30 sccache hits in 286 compiles (cause not analysed). Seeding by reflink
  made a second worktree at the same commit build in 1 s; a plain copy cost 36 s to save 24 s.
- Goal G4 of design.md § 2 is an overhead under 1 s per call for a warm worktree.

### 2.3 What today's remote mode does not do

1. **No pool across hosts.** The client decides alone, from its own environment
   (`client.rs:185-209`). The local daemon never hears of a remote job: it is not in the local
   queue, the web view, `slot usage` or the local job log.
2. **One host per client, every command.** `SLOT_HOST` names one host and applies to every `slot`
   call that sees it: `slot task check` and `slot npx playwright test` go remote too unless
   `.slot.toml` lists them as `local`. There is no choice by load and no health check.
3. **No lifecycle.** Nothing starts, drains or stops a host. A host's job log ends with the host.
4. **A lost start is not retried.** Only a failed sync falls back. When the sync succeeds and the
   host daemon is down, or the link drops before the job starts, the client exits 3
   (`client.rs:190-192`); the resubmit of an unstarted job is for the local daemon only
   (`client.rs:454-466`).
5. **No keepalive.** The default ssh options (`client.rs:119-129`) set no `ServerAliveInterval`, so
   a silently dead link is noticed only when TCP gives up *(unverified how long that takes)*.
6. **The PATH shim does not work remotely.** `contrib/slot-shim:57` runs `slot run -- "$real" …`
   with the absolute path of the workstation's `cargo`. The host runs that path as given
   (`server/mod.rs:4816-4819`), so the job ends with 127 unless the same path exists there
   *(unverified: read from code, not run)*. `slot cargo …` typed in full sends the bare name.
7. **Gaps in the push** (§ 4.4) and **in the return** (§ 5). Target-directory paths in output
   (`Running … (<cache-root>/target/<id>/…)`) are not rewritten: only the mirror path is
   (`server/mod.rs:1649-1653`). The sync passes no `-z` (`client.rs:616-622`), although `slot
   shell` admits it (`shell.rs:58`, `:70`); `ssh -C` in `SLOT_SSH` compresses everything today.

## 3. Target: one pool of several hosts

```
workstation                                          cloud host(s), ephemeral
-----------                                          ------------------------
session ── slot cargo test -p x
  │ 1. request ──unix socket──► slot serve (local)
  │                              placement: local | host H
  │ 2. placement frame ◄──────── (counts the worktree against H)
  │ 3. rsync tree ─────────────────────ssh, no inbound port──────► <ws-root>/<id>/
  │ 4. ssh H slot proxy ───────────────────────────────────────► slot serve (H): queue, pool
  │                                                               cargo test -p x
  │                                                               CARGO_TARGET_DIR=<cache>/target/<id>
  │ 5. output, exit ◄──────────────────────────────────────────── sccache (H-local disk)
  │ 6. end report ──unix socket──► local job log: host, sync_ms, exit
  │
  slot serve (local) ── every 5 s: ssh H slot shell state ──► alive, version, clock, ended jobs
```

### 3.1 Who decides

Two shapes were considered.

- **(A) The daemon places, the client carries.** The client always asks the local daemon. The
  daemon answers as today (the job runs locally) or with a placement frame naming a host. The
  client then syncs and talks to that host itself, as remote mode does now, keeps its local
  connection open until the job ends, and reports sync time and exit on it.
- **(B) The daemon proxies.** The local daemon runs rsync and the ssh bridge and relays frames.
  It sees everything first-hand, but every remote job's pipes then live in the daemon: a reload
  must hand them over, a daemon crash ends every remote job, and the daemon relays every byte.

**Recommendation: (A).** The data path stays in the client, where it is built and measured; the
daemon gains a decision and a record, not a transport. Details that make (A) sound:

- The request carries a new field saying the client can follow a placement (`#[serde(default)]`,
  as `AGENTS.md` § Upgrading a running daemon asks). An older client never gets a placement frame.
- The local connection is an ordinary client socket: a client that dies frees the placement count
  as it frees a local slot, and the handover of design.md § 15.2 passes it on.
- When the sync or the start fails, the client says so on the same connection and the daemon
  queues the request locally with its first enqueue time. No second submit, no lost queue place.
- `SLOT_HOST` set by the caller keeps today's behaviour and bypasses placement. It is the tool of
  phase 0 and of debugging.

### 3.2 Placement is per worktree

A worktree builds where its target directory is. A job that moves between the workstation and a
host finds a target directory that is behind by every change built on the other side, and the
workstation keeps a second 13 to 25 GB directory, which is the disk this design frees. So the
decision is made once per worktree and kept for the host's life.

The local daemon applies these in order; the first that matches decides.

1. Local, as today: `slot --local`, a nested job (`client.rs:161-163`), a gated job
   (`client.rs:181-184`), a `.slot.toml` `local` prefix (`repo_config.rs:11-14`), a directory that
   is not a git checkout (`client.rs:607`).
2. Local, new: the built-in local list (§ 6), and every toolchain but rust and go.
3. Local: the repository is not opted in. The opt-in is the operator's and lives in the local
   daemon's state (`slot host allow <repository>`), not in `.slot.toml`: any session can write a
   file in its worktree, and a new `.slot.toml` key breaks older clients (§ 10 risk 8).
4. The worktree has a placement on a host that is `ready` and whose time left exceeds the job's
   run limit (`run_limit::job_limit`, `run_limit.rs:197-199`; on the workstation 10 minutes, at
   most 30): the job goes there.
5. The worktree has no placement and a `ready` host holds fewer than its `max_worktrees` (a
   number per host in `hosts.json`; default half its vCPUs, an estimate to tune in phase 0): the
   worktree is placed there.
6. Otherwise local.

Rejected for phase 1: placing each job by the free slots of both sides, with short jobs kept
local. A free-slot count is not a job count (a cargo job holds one token and draws more,
`toolchain.rs:199-201`), a number read every 5 s is stale under a burst, and a job's predicted
run time (`admission.rs:131-136`) comes from runs against a warm target directory, which the
other side does not have. The cost: a placed worktree queues on the host even while the
workstation idles. Phase 3 may move a worktree by measured load.

### 3.3 Slots per host

A jobserver FIFO cannot span machines, so each host keeps its own `slot serve` and its own pool,
sized to its CPUs. The local daemon hands out no host tokens. It bounds worktrees per host
(§ 3.2 rule 5); the host's queue, its memory and disk admission (design.md § 14) and its run
limit order what arrives.

### 3.4 Host registry and health

- `<state-dir>/hosts.json` lists each host: name, ssh command, path of `slot` on the host,
  provider instance id, slot version, deadline, price per hour, `max_worktrees`, state (`joining`,
  `ready`, `draining`, `gone`), and the placed workspace ids.
- The local daemon polls each host every 5 s with a new `slot shell` step, `state` (`slot shell`
  knows only `Rsync`, `Proxy` and `Cancel` today, `shell.rs:31-40`). The answer carries the host
  daemon's version, its clock, whether it admits jobs, and the job-log lines since the last poll,
  which the local daemon appends to `<state-dir>/hosts/<name>/jobs.jsonl`. A reclaimed host then
  loses at most one poll of its log, and no key needs a fetch path for the log.
- Three missed polls in a row, or three failed syncs in a row: `gone`; its placements end.
- Refused at join: a slot version other than the workstation's; a clock more than 1 s from the
  workstation's (§ 4.5).

### 3.5 Failure modes

| event | today | proposed |
|---|---|---|
| sync fails: host unreachable, host disk full, key refused | local daemon, one stderr line (`client.rs:200-207`) | the same, on the open connection with the first enqueue time (§ 3.1) |
| sync done, job never starts: host daemon down, link drops, ssh refuses the session | exit 3 (`client.rs:190-192`) | treated like a failed sync: the job runs locally |
| link drops mid-job, host alive | exit 3; the host cancels the job when its sshd sees the dead link | `ServerAliveInterval 15` and `ClientAliveInterval 15`, 3 misses, on both ends; then as the next row |
| host lost mid-job: spot reclaim, crash | exit 3 | the client runs the job once more on the local daemon and says so in one line; `SLOT_NO_RESUBMIT=1` turns it off |
| spot notice, 2 minutes ahead | nothing | a root unit on the host watches the metadata service and closes the host daemon's admission; the next poll sets `draining`; jobs that end within the notice finish |
| the shared ssh control connection dies | every job of that host loses its link at once | as "host lost"; a control connection per session bounds it to one session *(unverified option)* |
| workstation sleeps, reboots, or its daemon stops | clients die, the host cancels their jobs | the host stops itself when idle for 30 min and at its deadline (§ 7.4); nothing depends on the workstation |
| local daemon reloads | remote jobs are not affected: the remote leg is client to host | placements travel in the handover state (§ 3.6) |
| host reaches its deadline | — | `draining` from the deadline minus the longest run limit (30 min); placements end |

A rerun after a lost host is new: today a job that had started is never sent again (`AGENTS.md`
§ Upgrading a running daemon). It is acceptable here because the first attempt ran in a mirror,
not in the local tree, and a job with effects outside its tree is not placed (§ 6). During a
partition the first attempt may run on for up to 45 s beside the rerun.

What a lost host costs: every job on it arrives in the local queue at once, each against a local
target directory that is stale or evicted. The local disk and memory admission (design.md § 14)
is the brake; the workstation is then where it is today.

### 3.6 Reload and handover

- Local daemon reload: the client's local connection is handed over (`server/handover.rs`). The
  placement of each open connection and the placed workspace ids are added to the handover state
  with `#[serde(default)]`. A binary that does not know the field is not handed over to while a
  placement is open: the version check that today refuses a binary older than 0.11.0 (design.md
  § 15.2) gains that case, and the bounded hold of design.md § 15.1 applies.
- Local daemon crash: remote jobs run on; the restarted daemon undercounts placements until they
  end. The host's own queue is the limit that matters.
- Host daemon: hosts are replaced, not upgraded. A host runs one slot version from boot to end.

### 3.7 The job log, the view, and what a session sees

- New fields on the local job, all `#[serde(default)]`: `host` (empty for local), `sync_ms`,
  `fetch_ms`, `remote_id` (the job's id on the host), `rerun_of`. CPU seconds and peak memory of
  a remote job are in the host's log lines, which the poll copies (§ 3.4); `remote_id` joins them.
- `slot status` and the web view show the host per job and one line per host: state, placed
  worktrees, running and queued jobs, deadline, cost so far.
- A session sees one stderr line per remote job, `slot: job 812 on build-1, synced in 0.4 s`,
  and one line when a job fell back or was rerun. After a remote `cargo build` without `--fetch`
  the client adds `slot: built on build-1; target/ here is unchanged (--fetch <path> copies a
  file back)`: the stale local binary of § 5.1 is the likeliest way an agent is misled.
- Host target-directory paths in output are rewritten to `target/` (§ 2.3 item 7), so no line
  names a path that does not exist locally.

## 4. Code transfer

### 4.1 Options

| | (a) push the tree (rsync over ssh) | (b) host clones with a read-only deploy token | (c) push a git bundle |
|---|---|---|---|
| uncommitted changes | included (agents build uncommitted state almost always) | missing; needs a patch on top | missing; needs a patch on top |
| credentials on the host | none | a token per repository or one for the organization | none |
| first transfer | the tree, once per worktree | full history from the git host, fast inside the cloud | full history once, then deltas |
| repeat transfer | changed files only | `git fetch` plus the patch | new commits plus the patch |
| `.git` on the host | no (excluded) | yes | yes |
| path dependencies outside the repository | missing (fixable, § 4.4) | missing | missing |
| works with worktrees | yes, one mirror per worktree | yes | yes |
| built | yes (`client.rs:615-634`) | no | no |

### 4.2 Recommendation: (a), extended

Push the tree. It is built and measured, it carries the state the agent is building, and it puts
no credential on the host. (b) puts a long-lived read credential on a machine that runs build
scripts of every dependency, and still needs a patch for uncommitted work. (c) needs the same
patch, and its first transfer is the whole history. What (b) and (c) add, a `.git` directory, is
needed only by build scripts that call git (§ 4.4).

### 4.3 Uncommitted changes and worktrees

Every file in the worktree that the `.gitignore` filter does not exclude is sent: tracked,
modified and untracked. Each worktree has its own workspace id and mirror (`client.rs:571-577`).
`--delete` makes the mirror equal to the tree at each sync, so what a remote job wrote into the
mirror outside an excluded directory is gone at the next sync (§ 5.5). A sync while another job
of the same worktree runs changes that job's sources under it, as an edit does to a local build.

### 4.4 What the push misses or gets wrong

| gap | cause | effect | fix |
|---|---|---|---|
| path dependency outside the repository (`path = "../shared"`) | only the repository root is synced (`client.rs:629-630`) | build fails on the host | phase 0 to 2: such repositories stay local. Phase 3: the client reads `cargo metadata`, syncs each out-of-tree package root and lays the mirror out so the relative paths hold |
| `.git` | excluded (`client.rs:621`) | build scripts that call `git` (version strings) fail or differ | pre-flight lists them; the script tolerates a missing `.git`, or the repository stays local |
| gitignored build inputs | the `.gitignore` filter (`client.rs:620`) | in this repository `crates/slot/build.rs:23-36` embeds `ui/dist`, which `.gitignore` excludes, and writes a placeholder page when it is missing: a remote build of slot has no web UI | phase 2: the operator's opt-in names ignored paths to include |
| rsync reads `.gitignore` by its own rules | `--filter=':- .gitignore'` treats each line as an rsync exclude | a `!keep` line does not re-include a file, so it is missing on the host; files ignored only by `.git/info/exclude` or the user's global ignore file are sent; a tracked file that matches an ignore line is not sent *(unverified: from rsync's filter rules, not run)* | pre-flight: compare `git ls-files --cached --others --exclude-standard` with `rsync --list-only` of the same tree. Phase 0 compared them on a tree of 10,540 files: the filter selected exactly git's list, 115,985,019 bytes both ways, so sending the git list with `--files-from` saves no bytes there and is not built; it stays the fix for a tree where the lists differ |
| a nested `.git` | `--exclude=/.git` is anchored at the root | a clone inside the worktree crosses with its `.git/config`, which may hold a token in a remote URL | phase 1: `--exclude=.git` at every depth |
| symlinks that leave the tree | `rsync -a` copies links as links | the link dangles on the host | pre-flight lists them; such repositories stay local |
| the command's absolute path | the shim sends `$real` (§ 2.3 item 6) | exit 127 on the host | phase 0: type `slot cargo …`. Phase 1: for a placed job the client sends the file name of `argv[0]` |
| environment values that are workstation paths | every `RUST*` and `CARGO_*` name crosses but three (`toolchain.rs:213-219`): `RUSTC`, `RUSTC_WRAPPER`, `RUSTUP_HOME`, `CARGO_INSTALL_ROOT` when set | the host job looks for a path it does not have | phase 0: check `env` in the session. Phase 1: hold back a value that is a path under the caller's home |
| the workstation's `~/.cargo/config.toml` (sccache wrapper, `debug = "line-tables-only"`, linker) | not part of any repository | the host builds with other settings | the host image writes the same file (§ 7.3). `target-cpu=native` must not be in it: a fetched binary could use instructions the workstation lacks |
| private git dependencies, private Go modules | the host fetches what `Cargo.lock` or `go.sum` names; `CARGO_HOME`, `GOPATH` and `GOMODCACHE` stay the host's (`toolchain.rs:213`, `:263-268`) | fetch fails without a credential | phase 0 to 2: stay local. Phase 3: the client syncs the locked git databases into the host's cargo home *(unverified that cargo then skips the fetch)*; no credential crosses |
| registry crates | the host's own `CARGO_HOME` | the first build downloads them | nothing: downloads inside a cloud are fast. A baked image can carry a warm registry |
| `CARGO_MANIFEST_DIR` and `file!()` in binaries | the mirror path differs from the workstation path | a fetched binary that reads `env!("CARGO_MANIFEST_DIR")` at run time looks for host paths; debug info and panic locations name host paths | § 5.7 |

### 4.5 Modification times, clocks and file owners

`rsync -a` keeps each file's modification time, and cargo decides whether a crate is fresh by
comparing those times with the time of its own outputs. design.md § 16 shows the mechanism: a
seeded target directory gave "0 units (sources older than the copied outputs)". Three things
follow.

- **Clock skew.** If the host's clock is ahead of the workstation's by *d*, a file edited within
  *d* after a remote build is older than that build's outputs, so cargo reports it built and the
  job exits 0 on old code. If the host is behind, crates rebuild until its clock passes the
  edit. Machines on NTP differ by milliseconds, so the join check of § 3.4 (1 s) suffices
  *(unverified: reasoned from cargo's fingerprints; phase 0 reads both clocks)*.
- **A seeded target directory from another commit.** The seed is the most recently used target
  directory of the same repository (`server/cache.rs:156-165`), whatever commit built it. A
  worktree whose changed files are older than the seed's outputs would then be reported fresh,
  with the other worktree's artifacts. design.md § 16 measured seeding only between worktrees
  at one commit; the pool brings 24 worktrees at different commits to one host *(unverified:
  not reproduced; phase 0 step 7 tests it)*. If it reproduces: no seeding until fixed (a cache
  root without reflinks skips it, `server/cache.rs:172-184`); the fix drops the fingerprints
  of the workspace's own packages from a seed, so only dependency artifacts are reused.
- **Owners and modes.** Files arrive owned by the host's build user and keep their modes; a
  fetched file is owned by the caller. User ids need not match.

### 4.6 Size and time

Assumptions, because nothing here is measured over a cloud link: uplink 20 Mbit/s (cable) or
100 Mbit/s (fibre); downlink 100 to 250 Mbit/s; round trip 20 to 40 ms to the nearest region;
source text compresses about 3 times. Time = megabytes x 8 / megabits per second.

| what | size | 20 Mbit/s up | 100 Mbit/s up |
|---|---|---|---|
| this repository's tracked files (738 files, 17,598,591 bytes by `git ls-files` and `du`) | 17.6 MB | 7 s, ~2.5 s compressed | 1.4 s, < 1 s compressed |
| the 66-crate workspace of design.md § 16 (9,199 files) | 132 MB | 53 s, ~18 s compressed | 11 s, ~4 s compressed |
| 24 worktrees of that size, first sync each, no seeding | 3.2 GB | 21 min, ~7 min compressed | 4.2 min, ~1.5 min compressed |
| a sync with one file changed | a file list of ~9,000 entries (estimated 0.5 MB) plus the file | estimated 0.5 to 1.5 s | estimated 0.5 to 1 s |

The first sync of a worktree is a one-time cost per host life; repeat syncs decide G4. Over a
cloud link the repeat sync is estimated at 0.5 to 1.5 s against 0.29 s on loopback: the file-list
exchange and a few round trips. The estimate assumes the tunnel of § 8.1 passes the link's full
speed, which is not known for the recommended one. Phase 0 measures both.

### 4.7 A new worktree on a host that has its siblings

Agents create a worktree per change, so a host sees many worktrees of one repository near one
commit. Phase 0 on a cloud host measured why this matters: the first sync of each worktree crossed
a tunnel of about 1 MB/s, and a 145 MB tree took more than 16 minutes.

Built, in a form other than the reflink copy first planned here: before every sync the client
runs `slot __siblings <ws-root> <id> <key>` on the host (`sync.rs`, `mirror.rs`). `<key>` is the
first 8 bytes of SHA-256 of the normalized `origin` URL (host and path, lower case, without
scheme, user, port and `.git`), so every clone and worktree of one repository shares it; without
an `origin` it hashes the workstation's name and the common git directory. The step writes the
key to `<ws-root>/.repo-keys/<id>` and prints up to four other ids with the same key whose
mirror exists, the most recently recorded first. rsync gets each as `--copy-dest=../<id>`: for a
file missing in the new mirror it copies a sibling's file on the host when size and time match,
and otherwise takes the sibling's file as the delta base. Worktrees checked out at different
times differ in every file's time, so the second case is the common one; what crosses is the
file list, block checksums and changed blocks. Measured by the test
`a_new_worktree_sends_only_what_differs_from_its_sibling_mirror`: a worktree of a 2 MiB tree at
the same commit as a synced sibling sent under 5 % of the tree.

- Why `--copy-dest` and not a copy on the host: no new copy step that could stall on its own, no
  reflink needed on the host's filesystem, and rsync can draw on several siblings. `slot shell`
  admits `--copy-dest` only as `../<id>` in a push, so it reads nothing outside the workspace
  root.
- The step runs before every sync, not only the first: a mirror whose first sync was killed
  still gets its siblings, and `--copy-dest` costs nothing for files the mirror already has.
- A host whose slot does not know the step refuses it (`slot shell`, exit 126): the client then
  sends the plain push without `--copy-dest` and `--stats`, which that host would refuse too.
- Eviction removes a mirror's key with the mirror (`server/cache.rs`).

Each sync also prints `slot: synced to <host> in <s> s, <n> bytes sent[, <k> sibling mirrors as
base]`, the bytes from rsync's `--stats`. And a sync is bounded: rsync's read and written bytes
(`/proc/<pid>/io`) are sampled every 100 ms; no change for `SLOT_SYNC_STALL` (60 s) or a run past
`SLOT_SYNC_LIMIT` (5 min) kills rsync and its ssh, and the job goes to the local daemon as for a
failed sync. A killed rsync never leads to `slot proxy` on the host.

The target directory seeding of § 4.5 is unchanged: it still copies the newest target directory
of the same repository name by reflink.

## 5. Result transfer

### 5.1 The default: output and exit code

A remote job returns its stdout, stderr and exit code, and nothing else (`client.rs:190-198`).
That is right for `cargo check`, `clippy`, `test`, `nextest` and a `build` used as a check.

The trap is a `build` whose product is run next. The local `target/` is unchanged, so
`./target/debug/mytool` is absent or, worse, the product of an older local build. Phase 1 prints
the line of § 3.7; phase 2 lets the operator's opt-in name files that every successful `cargo
build` of the repository fetches.

### 5.2 `cargo build` of a binary that runs locally

`slot --fetch target/debug/mytool cargo build -p mytool` (`client.rs:264-298`). The path is
relative to the repository root; `target/…` is read from the job's target directory on the host,
anything else from the mirror (`client.rs:230-260`). Sizes decide the cost:

| artifact | size | 100 Mbit/s down | 250 Mbit/s down |
|---|---|---|---|
| debug CLI binary (design.md § 16) | 410 MB | 33 s | 13 s |
| release CLI binary, stripped (typical, an estimate) | 5 to 30 MB | under 3 s | about 1 s |

rsync's delta transfer gains little on a relinked binary *(unverified)*. For an edit-build-run
loop on a large binary, fetching it each time costs more than the build saved: such loops build
with less debug info (`CARGO_PROFILE_DEV_DEBUG` crosses, `toolchain.rs:216`) or stay local.
Cloud egress costs about $0.09 per GB after a free allowance (approximate): $0.04 per 410 MB.

Phase 3: `--fetch auto`. The host lists the files in `target/<profile>/` (not `deps/`) that the
job created or changed and the client fetches those, after checking each name (§ 8.6).

### 5.3 `cargo test`

Only the log and the exit code come back. Tests that read files in the repository work, since
the tree is synced. Tests that need anything outside it (a local service, docker, a socket, a
file in the home directory, a kind cluster) fail remotely and belong in `.slot.toml` `local`.

What a failing run leaves behind stays on the host, because the fetch runs only after exit 0
(`client.rs:194`): a JUnit report, a coverage file, `*.snap.new`, a `proptest-regressions` file.
Phase 2: the operator's opt-in names `return` patterns, and files a job created under them come
back whatever the exit code.

### 5.4 `cargo run`

`cargo run` on the host runs the program on the host: wrong for a program that talks to local
files, sockets, a browser or a terminal. It stays local, and builds into the local target
directory at local cost. Phase 3 may split it: a remote `cargo build` with the same options and
`--message-format=json-render-diagnostics` to learn the executable's path, a fetch of that one
file, and a local exec with the arguments after `--`.

### 5.5 Files a job writes into the tree

A remote job writes into the mirror, and the next sync deletes or reverts it (`--delete`, § 4.3).

- **Stay local** (built-in local list, § 6): `cargo fmt`, `fix`, `clippy --fix`, `update`, `add`,
  `insta accept` and `review`, generators (`task generate`).
- **Bring back on the side** (phase 2): `Cargo.lock` when the job changed it, for example after
  a dependency was added by hand. Without this the workstation's lock file stays old, the host
  resolves again on every job, and a later `--locked` build fails. The host compares the file's
  digest before and after; the exit frame names it; the client fetches it and says so.
- **Bring back on request** (phase 2): the `return` patterns of § 5.3.

### 5.6 Target directories and sccache stay on the host

- **Never synced back.** A target directory of 15 GB takes 20 min at 100 Mbit/s, costs about
  $1.35 in egress, is rewritten in large part by each build, and would put 13 to 25 GB per
  worktree back on the workstation's disk.
- **Per worktree, on the host.** As today: `<cache-root>/target/<id>` (`toolchain.rs:205-208`),
  seeded by reflink from a sibling (subject to § 4.5), evicted least recently used together with
  its mirror (`server/cache.rs:197-205`). On an ephemeral host they end with the host. The cache
  root is local NVMe formatted xfs, since seeding pays only with reflinks (design.md § 16 item 4).
- **sccache, one server per host**, on the host's NVMe, with a store size set for that disk.
- **Across workspaces the hit rate is poor and the cause is open.** design.md § 16 saw 30 hits in
  286 compiles between two workspace ids on one machine. One candidate: slot sets
  `CARGO_TARGET_DIR=<cache-root>/target/<id>` in the job's environment, and that value differs
  per workspace and reaches every rustc call *(unverified that sccache's key includes it)*. A
  `target` link in the mirror that points at the cache directory would remove the variable.
- **Across host lives**, phase 3: sccache's object-store backend, one bucket in the host's
  region. It helps dependency crates only; workspace crates built incrementally are not cached
  (design.md § 1 item 3), and it helps at all only once the point above is understood.
- **The workstation's cache** is not shared: uploading costs the uplink, and its keys contain
  workstation paths.

### 5.7 Should the mirror sit at the workstation's absolute path?

Today the mirror is `<ws-root>/<id>` and slot rewrites that prefix in output. What remains
host-shaped: debug info, panic locations and `file!()` in a fetched binary, and
`env!("CARGO_MANIFEST_DIR")` read at run time.

| option | fixes | costs |
|---|---|---|
| (1) keep `<ws-root>/<id>` | nothing more | a debugger needs a path substitution for a fetched binary; panics in it print host paths |
| (2) `--remap-path-prefix` in `RUSTFLAGS` | debug info, panics, `file!()` | not `CARGO_MANIFEST_DIR`; the flag differs per worktree, so it is part of every cache key and no worktree shares a cached crate with another |
| (3) run each job in a private mount namespace that binds the mirror at the workstation's path and the target directory at `<that path>/target` | everything: paths in binaries and output equal local ones, the output rewrite goes away | needs unprivileged user namespaces on the host and a wrapper around each job; workstation directory names appear in the host's process list |

Recommendation: (1) through phase 2, since binaries that are fetched and debugged are the rare
case. Evaluate (3) in phase 3 *(unverified: not prototyped)*. (2) is rejected for its cache cost.
None of the three makes two worktrees share sccache entries: their paths differ on the
workstation too.

## 6. Jobs that stay local

| class | why | how slot knows |
|---|---|---|
| needs local services: docker, a database, a kind cluster, a port on localhost | not on the host | `.slot.toml` `local` (built) |
| writes into the tree: `fmt`, `fix`, `--fix`, `update`, `add`, `insta accept`, generators | the result is lost at the next sync | built-in list (new), extendable in `.slot.toml` `local` |
| `cargo run`, `go run` | runs the program in the wrong place | built-in list (new), until § 5.4 |
| `cargo install`, `go install` | installs on the host; the workstation gets nothing | built-in list (new) |
| `cargo doc --open`, `cargo bench` | opens a browser on the host; measures another machine | built-in list (new) |
| `cargo publish`, `package`, and builds of published artifacts | releases are built by CI from a tag (`AGENTS.md` § Release), never by the pool | built-in list (new) |
| node test runners (`toolchain.rs:296-308`), browser and UI gates | `node_modules` is not synced (`toolchain.rs:375-377`), the host has no Chromium and not the fonts the UX gates measure with, and reports are written into the tree | toolchain is not rust or go (§ 3.2 rule 2) |
| wrappers: `task`, `make`, `just`, `pnpm` (toolchain `other`) | inner cargo calls see `SLOT_JOB` and run inside the job (`client.rs:161-163`), so a wrapper sent remote takes every step with it, node and browser included | toolchain is not rust or go |
| a directory outside a git checkout | remote mode needs a repository root (`client.rs:607`) | built |
| GPU | the suggested hosts have none | `.slot.toml` `local` |

**Go** goes remote with rust. `go test`, `go vet`, `go build` and `golangci-lint run` are placed;
`go generate`, `go mod tidy`, `go get` and `go fmt` are on the built-in local list. All
worktrees share one build cache on the host (`GOCACHE=<cache-root>/go-build`,
`toolchain.rs:255-257`), which is keyed by content, so a new worktree starts warm *(unverified on
a host)*. The module cache is the host's: public modules download there, private ones keep the
repository local (§ 4.4). `go build` in a main package writes the executable into the mirror;
`--fetch <name>` in the same call brings it back before the next sync deletes it. The host needs
the Go version of `go.mod` and a C toolchain for cgo.

**Node** stays local in every phase of this plan. Sending it needs an install per mirror, the
browsers and the fonts. It is worth a design only if the job log shows node tests holding a large
share of the workstation's slot-hours once rust has left.

## 7. Ephemeral host lifecycle

### 7.1 Instance choice

x86_64 only: a fetched artifact must run on the workstation. Local NVMe (the `d` families) over
network block storage: target directories do heavy small-file I/O, and instance storage ends
with the host, which this design wants.

Approximate on-demand prices, Linux, one US East region, public list prices as remembered on
2026-11-23; other regions are often about 10 % higher, spot often 40 to 70 % lower. All of it
to check before use.

| type | vCPU | RAM | local disk | on-demand, approx. $/h | 4 h | 8 h |
|---|---|---|---|---|---|---|
| c6id.8xlarge | 32 | 64 GiB | 1 x 1.9 TB NVMe | 1.6 | 6.5 | 13 |
| c6id.16xlarge | 64 | 128 GiB | 2 x 1.9 TB NVMe | 3.2 | 13 | 26 |
| c6id.32xlarge | 128 | 256 GiB | 4 x 1.9 TB NVMe | 6.5 | 26 | 52 |
| m6id.16xlarge | 64 | 256 GiB | 2 x 1.9 TB NVMe | 3.8 | 15 | 30 |
| c7a.16xlarge (one core per vCPU, no local disk) | 64 | 128 GiB | network storage only | 3.3 | 13 | 26 |

**Suggestion: c6id.16xlarge.** 64 vCPUs against the workstation's 20 CPUs. On this family a vCPU
is a hyperthread (32 cores), so the gain in throughput is not 3.2 times and phase 0 measures it.
Memory admission counts jobs, not slots: at the 3.9 GiB that design.md § 14 gives as the 90th
percentile of a rust job, 24 jobs at once need about 94 GiB of the 128. 3.8 TB of NVMe holds
the target directories of 24 worktrees at 13 to 25 GB each (300 to 600 GB). Choose m6id when the
host's job log shows jobs waiting for memory.

### 7.2 Spot or on-demand

Phase 0 and 1 use on-demand, so an interruption does not spoil the first measurements. From
phase 2: spot, with a list of equivalent types and the price cap at the on-demand price. An
interruption costs each running job one rerun (§ 3.5) and the host's warm target directories.
On-demand is the fallback when no spot capacity is found within a minute.

### 7.3 Image and bootstrap

A boot script (POSIX sh, in user-data), later a baked image. It runs as root and does this:

1. `shutdown -h +<minutes>` first. With the instance's shutdown behaviour set to terminate, the
   host ends at its deadline even if the workstation is off or the teardown fails (§ 8.7).
2. Format the NVMe disks as xfs and mount them at `/srv/slot`. Everything that grows goes there:
   the slot state directory, `CARGO_HOME`, `RUSTUP_HOME`, the Go module cache, the sccache store
   and the jobs' `TMPDIR`. The root volume is small, and tests write temporary files.
3. Install rsync 3.4 or later, a C toolchain, and the linker the workstation's cargo config names.
4. Create the build user without sudo and enable lingering for its systemd user units.
5. Block the build user from the metadata service with a firewall rule on its user id (§ 8.3).
6. sshd: `MaxSessions 200`, `ClientAliveInterval 15`, `ClientAliveCountMax 3`. Every remote job
   holds one session on the shared connection for its whole run, and sshd's default of 10
   sessions per connection would refuse the eleventh *(unverified: sshd's documented default)*.
7. Install the slot release of the workstation's version and check it against the release's
   `SHA256SUMS` (`AGENTS.md` § Release).
8. Install rustup with every toolchain named by the `rust-toolchain.toml` of the opted-in
   repositories, and Go if one needs it. Install sccache as one service.
9. Write the build user's `~/.cargo/config.toml` with the workstation's build settings (§ 4.4).
10. Link the build user's `~/.local/state/slot` to `/srv/slot/state`, so the client's default
    workspace root (`paths.rs:44`) and the daemon's (`cli.rs:705-712`) agree and sit on NVMe.
11. Start `slot serve` with floors for the host's size (design.md § 16 items 2 and 6), for
    example `--min-free-disk 50 --min-free-memory 8`, a raised open-file limit in its unit (a
    remote job gets the host daemon's limit, `client.rs:214-217`) and the workstation's run limit.
12. Start two root units: the idle watchdog (no job for 30 minutes: `shutdown -h now`) and, on
    spot, the interruption watcher of § 3.5.
13. Install the per-host public key with the forced command (§ 8.2). Check the clock is synced.

Estimated time to ready: 2 to 4 minutes from launch; a baked image (phase 3) under one minute.

### 7.4 Join, drain, stop

- **Join.** The host is `joining` until the `state` step answers with the expected version, a
  clock within 1 s and an open admission; then `ready`.
- **Drain.** At the deadline minus the longest run limit a job may name (30 min), on `slot host
  drain`, or on a spot notice: no new placements; running jobs finish.
- **Stop.** When drained and idle, or at the deadline: terminate through the provider, delete
  the key directory and the account-side schedule (§ 8.7). The job log is local already (§ 3.4).
- **Idle.** The host stops itself after 30 minutes without a job (§ 7.3 step 12).
- **Disk.** The host daemon's `--cache-budget` and disk floor evict the least recently used
  target directories and mirrors (`server/cache.rs:197-205`). A host whose disk fills anyway
  refuses syncs, and those jobs fall back to the local daemon (design.md § 16 item 5).

### 7.5 CLI sketch, and who starts a host

```
slot host add    <name> --ssh "<ssh command>" [--remote <path of slot>] [--max-worktrees N]
slot host allow  <repository>                  # the operator's opt-in (§ 3.2)
slot host list                                 # state, worktrees, jobs, deadline, cost so far
slot host drain  <name>
slot host remove <name>
slot host up     --hours 4 [--type c6id.16xlarge] [--spot] [--max-cost 20] [--name build-1]
slot host down   <name> [--now]
```

The first five are phase 1; `up` and `down` are phase 2. HTTP: `GET /api/hosts`, `POST
/api/hosts/<name>/drain`, `POST /api/hosts/<name>/down`; an older client gets 404, as for every
unknown `/api/` path today.

The cloud call is not in slot's binary. `slot host up` runs a provider program
`slot-host-<provider>` from `PATH`: `up <name> <type> <hours> <spot> <public-key-file>
<toolchains>` prints one JSON line (`instance_id`, `ssh`, `price_per_hour`); `down
<instance_id>`; `list`. A POSIX sh provider that calls the cloud's CLI ships in `contrib/`.
slot stays a scheduler without a cloud SDK (`AGENTS.md` § Boundary).

A paid host is a spending decision: the operator starts it, for a night or a busy afternoon.
Automatic start by queue pressure comes only in phase 3 and only under a budget the operator
set beforehand, an amount per day and a most-hours-per-host, both in `hosts.json`.

## 8. Security

Goal: only the workstation reaches the host; the host holds nothing worth stealing beyond the
source it is given; a compromised host reaches no further than a compromised dependency built
locally could; the host cannot outlive its deadline whatever happens to the workstation.

### 8.1 Reachability

The host's security group has no inbound rule. The host sits in a public subnet with a public
address, so it reaches the registry, rustup, the release download and the access service over
outbound HTTPS without a NAT gateway and its per-GB charge *(approximate)*. Outbound traffic is
not filtered: a build script can send the source anywhere, on the host as on the workstation.

| option | inbound port | third party | notes |
|---|---|---|---|
| ssh over the cloud's session manager (`AWS-StartSSHSession` as ssh `ProxyCommand`) | none | the cloud provider only | access is an IAM decision, logged, scoped to instances with the pool tag; needs the session agent on the host and the session-manager plugin beside the cloud CLI on the workstation; throughput and setup time per connection *(unverified; users report throughput well under link speed)* |
| the cloud's instance-connect endpoint | none | the cloud provider only | a connection ends after about one hour *(unverified)*; that would cut the shared control connection and every job on it; rejected unless the check says otherwise |
| Tailscale with a tagged, ephemeral, single-use auth key | none (NAT traversal) | the coordination service; relays when no direct path is found | ACL: the workstation to the pool tag on port 22 only; the auth key travels in user-data (§ 8.2) |
| plain WireGuard | one UDP port, limited to the workstation's public address | none | a home address changes; an open port |
| ssh on a public port, limited by source address | 22 | none | rejected: a public port |

**Recommendation: ssh over the session manager, decided by measurement.** It has no port and no
extra party. Phase 0 step 5 measures it before any job is sent: the first sync of 132 MB and a
fetch of 410 MB must run at half the link's speed or better, and a job on a warm control
connection must gain under 1 s. If it fails either, use Tailscale. All ssh options go in one
config file named with `-F`, because `SLOT_SSH` is split on whitespace (`client.rs:134`) and a
`ProxyCommand` has spaces. That file sets `ConnectTimeout 10` (the default is 3 s,
`client.rs:128`, and a session is set up first) and the keepalives of § 3.5.

### 8.2 Keys

- One ed25519 key pair per host, made at launch in `<state-dir>/hosts/<name>/` (mode 600) and
  deleted at `down`. Its public half goes to the host in user-data with `command="<path>/slot
  shell",restrict` (`shell.rs:1-27`): no shell, no port or agent forwarding. This is not a
  sandbox: whoever holds the key runs any command as the build user through `slot proxy`.
- The host key is accepted on first use into a per-host `known_hosts` file. That is acceptable
  with the session manager, which ties the connection to the instance id; with Tailscale the
  launcher reads the host key from the instance's console log.
- No ssh agent forwarding, and no deploy key on the host (design.md § 5 item 2 proposed one).
- User-data is readable through the metadata service. It carries public keys only; with
  Tailscale also a single-use auth key, which the firewall rule of § 7.3 step 5 keeps from jobs.
- Phase 0 uses a second, unrestricted key for setup and for copying the job log. It lives only
  as long as the host. From phase 1 the `state` step replaces it.

### 8.3 Rights of the host

- Build jobs run as the build user and must not reach the instance's role. A metadata hop limit
  of 1 does not stop them: it stops containers, not processes on the host. Hence the firewall
  rule by user id (§ 7.3 step 5); the session agent runs as root and still reaches the service.
- Instance role: only what the session agent needs. The provider's managed core policy is
  broader: as remembered it allows reading the account's parameter store *(unverified; read the
  policy document)*. Use a policy written for the pool, or an account that holds nothing else.
  With the object-store sccache of phase 3 it adds read and write on one bucket prefix.
- Metadata service v2 only, hop limit 1.
- The workstation's cloud identity may: start instances from the pool's one launch template,
  pass the one instance role, terminate and describe instances that carry the pool tag, start
  sessions on them with the ssh session document, create and delete the schedule of § 8.7.
  Nothing else. It is a credential on the workstation and never crosses (§ 8.4).

### 8.4 What crosses the wire

To the host:

- **The tree** minus what the `.gitignore` filter excludes, the root `.git` and the toolchain's
  output directories (`client.rs:615-630`). Gaps: an untracked, not-ignored file crosses, for
  example a token file a session wrote next to the code; so do a nested clone's `.git/config`,
  a file only the user's global ignore file hides (§ 4.4), and a tracked `.cargo/config.toml`.
- **Phase 1 adds a deny list** to the sync (`.env*`, `.envrc`, `*.pem`, `*.key`, `id_*`,
  `.netrc`, `.npmrc`, `*.tfvars`, `*.tfstate`, `*credentials*`, `.git` at any depth) and prints
  each name it held back. It is best effort. The control that counts is the operator's opt-in
  per repository (§ 3.2 rule 3), given after reading the list of files (§ 9.1 step 1).
- **The command line**, in full. A secret passed as an argument crosses.
- **Environment names** the toolchain admits and that do not look like a credential
  (`client.rs:635-639`). Names with `TOKEN`, `SECRET`, `PASSWORD`, `PASSWD`, `CREDENTIAL`,
  `_KEY` or `AUTH` stay (`toolchain.rs:163-176`), and so do `HOME`, every `AWS_*` name and
  `CARGO_HOME`, which no toolchain admits (two tests, `toolchain.rs:740-777`). The value is not
  inspected: a proxy or registry URL with a password in it crosses under a name such as
  `CARGO_HTTP_PROXY` or `GOPROXY`. Phase 1 holds back a value that contains `://…:…@`. A name
  in `SLOT_ENV` crosses whatever it looks like (`client.rs:636`); that is the caller's choice.
- **Identity**: the session id, `hostname:pid` of the caller and the worktree's absolute path
  (`client.rs:170-175`, `:640-644`). They end up in the host's job log.

Never sent: `~/.cargo/credentials.toml`, ssh keys, the git configuration, the cloud identity.
From the host: output text, the exit code, fetched files. § 8.6 treats all three as untrusted.

### 8.5 Disk and termination

- The root volume is encrypted and deleted on termination; the launch template sets both.
  Instance-store NVMe is encrypted in hardware with a key that does not outlive the instance
  *(unverified; check for the chosen type)*. Nothing is snapshotted.
- Termination is the wipe: mirrors, target directories, the sccache store and logs end with the
  host. What can stay behind in the account: a volume not marked delete-on-termination, a
  schedule, a bucket (phase 3; give it an expiry rule). `slot host list` names every resource
  tagged for the pool that `hosts.json` does not know.

### 8.6 A compromised build host

Build scripts and tests run arbitrary code, so the host is as trustworthy as every dependency it
builds, and what it sends back is used on the workstation. design.md § 5 item 3 accepts the same
for a build server. The new trust roots are the cloud account and the image.

| what a compromised host can do | consequence | mitigation |
|---|---|---|
| return any bytes as a fetched binary | the workstation runs it as the user, with every credential the user has | equal to a compromised dependency built locally. Hosts are short-lived, from a pinned image and a checked slot release. Nothing published is built on the pool (§ 6) |
| return a symlink or a special file in a fetch | `rsync -a` keeps links (`client.rs:279-284`), so a fetched path can point outside the checkout | phase 1: fetch with `--no-links --no-devices --no-specials`; the client refuses what is not a regular file or directory |
| attack the workstation's rsync as a malicious sender | versions before 3.4.0 had flaws that let a sender write outside the destination *(unverified from memory of the advisories)* | rsync 3.4.0 or later on the workstation; checked at `slot host add` |
| choose what comes back by name (`--fetch auto`, `return` patterns, `Cargo.lock`) | a write anywhere the name allows: a changed `Cargo.lock` can point a dependency at another source | the client checks each name: relative, no `..`, matching a pattern the operator configured, never under `.git`, `.cargo` or `.github`, never `.slot.toml` or a build script. A returned `Cargo.lock` is reported by name, and its diff is the session's to read |
| write crafted text into the output | terminals obey control sequences; agents read output as context and may follow instructions in it | phase 2: the client drops control sequences other than colour from remote output. Agents treat build output as data, as they must for local builds |
| poison a shared cache | with the object-store sccache of phase 3, one host's bad object reaches later builds on every host | per-host sccache until phase 3; then a bucket per night, or the risk accepted in writing |
| read the source it is given, and send it out | the host sees every opted-in repository's tree | the opt-in and the deny list (§ 8.4) |
| cancel its own shutdown timer after gaining root | the host outlives its deadline | the account-side schedule of § 8.7 |

A full defence (the same commit built on two hosts and compared) needs reproducible builds,
which Rust does not give by default: an open question (§ 10), not a phase.

### 8.7 Teardown and the cost cap, independent of the workstation

Four layers, each working when the ones before it fail.

1. **In the host**: the shutdown timer of § 7.3 step 1 and the idle watchdog of step 12, both
   root's. The build user has no sudo. The launch template sets shutdown behaviour to terminate,
   and the workstation's identity can launch only from that template.
2. **In the account**: at launch, a one-time schedule in the provider's scheduler that calls
   terminate on that instance id at the deadline plus 5 minutes *(unverified that the scheduler
   can call the terminate API without code of ours)*. If creating it fails, the launcher
   terminates the instance at once. It works with the workstation off and the host compromised.
3. **On the workstation**: `slot host down`, the deadline in `hosts.json`, and `--max-cost`:
   divided by the price per hour it gives the most hours, and the shorter of that and `--hours`
   is the deadline. A daily cap in `hosts.json` refuses an `up` that would pass it.
4. **Alert**: a budget alert on the account. Billing data lags by hours, so it reports an
   overspend and does not prevent one.

Worst case: instance types are fixed by the launch template and the spot price is capped at
on-demand, but IAM does not cap the number of instances. The account's vCPU quota for the
family is the ceiling: read it, and lower it if the provider allows *(unverified)*.

## 9. Phased plan

### 9.1 Phase 0: one host by hand, `SLOT_HOST` per command

Cost to build: no code. About a day of setup and measurement, plus about $13 for 4 hours of an
on-demand c6id.16xlarge (approximate). Delivers: measured numbers for sync, overhead, cold and
warm builds over a cloud link, the answers to the *(unverified)* marks that matter, and relief
for the sessions that use it.

Runbook. Placeholders in angle brackets. Starting the host is the operator's spending decision.

1. Pick two to four repositories with the most build hours that pass the pre-flight:
   - no path dependency outside the repository root (`cargo metadata --format-version 1`:
     packages with no `source` whose manifest lies outside the root), no private git source in
     `Cargo.lock`, no build script that calls `git` or reads a gitignored file, no symlink that
     leaves the tree;
   - the list of files the sync sends holds no secret and equals `git ls-files --cached --others
     --exclude-standard`. The list:
     `rsync -a --list-only --filter=':- .gitignore' --exclude=/.git --exclude=/target/ <tree>/`
   - `env` in the session shows no `RUST*` or `CARGO_*` value that is a workstation path or a
     URL with a password (§ 4.4, § 8.4).
2. Once per account: a security group with no inbound rule; an instance role as § 8.3; a launch
   template with metadata v2 required, hop limit 1, shutdown behaviour terminate and an
   encrypted root volume deleted on termination; a budget alert.
3. Launch with the boot script of § 7.3 and a deadline of 240 minutes, then create the schedule
   of § 8.7:
   ```
   aws ec2 run-instances --launch-template LaunchTemplateName=<template> \
     --instance-type c6id.16xlarge --user-data file://<boot-script> \
     --tag-specifications 'ResourceType=instance,Tags=[{Key=slot-pool,Value=1}]'
   ```
4. On the workstation, an ssh config file for the pool:
   ```
   Host build-1
     HostName <instance-id>
     User <build-user>
     IdentityFile <key-dir>/build-1/id_ed25519
     IdentitiesOnly yes
     UserKnownHostsFile <key-dir>/build-1/known_hosts
     StrictHostKeyChecking accept-new
     ProxyCommand aws ssm start-session --target %h --document-name AWS-StartSSHSession --parameters portNumber=%p
     ControlMaster auto
     ControlPersist 10m
     ControlPath <runtime-dir>/slot-ssh-%C
     ConnectTimeout 10
     ServerAliveInterval 15
     ServerAliveCountMax 3
     Compression yes
   ```
5. Check the link before any build, with `SLOT_HOST=build-1 SLOT_REMOTE=/usr/local/bin/slot
   SLOT_SSH="ssh -F <config>"` in front: `slot cargo --version` twice (the first sync, then a
   warm call); one 400 MB file each way with the setup key; `date` on both machines. Apply the
   criteria of § 8.1.
6. Use. Put the three variables in a wrapper script of the operator's own, outside any
   repository, and call builds as `<wrapper> cargo test -p x`. Do not export `SLOT_HOST` in a
   session: every `slot` call would go remote (§ 2.3 item 2) and the PATH shim's `cargo` would
   fail on the host (§ 2.3 item 6).
7. Measure:
   - per job, from the host's `jobs.jsonl`: queue wait, run time, CPU seconds, peak memory; the
     client's wall time minus the host's enqueue-to-end is the overhead;
   - the first sync of each worktree, a no-change sync, a one-file sync, the fetch of a binary;
   - a cold build on 64 vCPUs against the same build locally;
   - seeding across commits (§ 4.5): two worktrees of one repository at different commits, the
     second created before the first one's build; build the first, then the second, which must
     compile every crate that differs;
   - sccache across workspace ids with and without `CARGO_TARGET_DIR` in the environment
     (§ 5.6), read from `sccache --show-stats`;
   - twelve jobs at once from different sessions, for the session limit of § 7.3 step 6.
8. Before the deadline: copy the host's `jobs.jsonl` back with the setup key; terminate the
   instance; delete the schedule and the key directory. Check with the provider's list that
   nothing tagged for the pool is left.

Phase 0 limits: no shared view; the local daemon does not know the host exists; each session
must be told; nothing keeps sessions from overloading the host while the workstation idles.

### 9.2 Phase 1: the daemon places worktrees on one host

Cost to build: roughly one week of agent work, an estimate. The host is still started and
stopped by the runbook, with its in-host deadline and the account-side schedule.

- `hosts.json`; `slot host add | allow | list | drain | remove`; the `state` step in `slot
  shell` and the poll with the log copy; the placement frame and the client following it; the
  rule of § 3.2; the end report and the job-log fields; the host shown in `slot status`.
- Failure handling of § 3.5: fallback on the open connection, an unstarted job run locally, one
  rerun after a lost host, the keepalives. Placements in the handover state (§ 3.6).
- What a session sees (§ 3.7), with the stale-binary line; target-directory paths rewritten.
- The built-in local list (§ 6); the file name of `argv[0]` for a placed job (§ 4.4).
- Security that costs little: the sync deny list and `.git` at any depth (§ 8.4), values held
  back by shape (§ 4.4, § 8.4), fetch of regular files only and the rsync version check (§ 8.6).
- Tests: each new `slot shell` step refused and admitted; the placement rule table-driven; the
  deny list; a handover with a placement open; a lost host with a stub ssh. No test calls a
  real cloud or the network. Every test stays under 60 s and the test step under 120 s.

Delivers: a build of an opted-in repository lands on the host without per-command variables;
one job log and one status; a host that leaves does not strand its jobs.

### 9.3 Phase 2: slot runs the host's life

Cost to build: roughly one to two weeks more, an estimate. Parts: `slot host up | down` with the
provider contract and a POSIX sh provider in `contrib/` (§ 7.5), the boot script, deadline, cost
cap, the account-side schedule and the orphan listing (§ 8.7); spot with on-demand fallback and
the interruption watcher; several hosts; a lane per host in the web view; `Cargo.lock` and
`return` patterns back, fetched files per repository (§ 5.1, § 5.3, § 5.5); ignored paths to
include and the git file list (§ 4.4); the output control-sequence filter (§ 8.6). Delivers: one
command for a host that cannot overspend or outlive its deadline; files that come back.

### 9.4 Phase 3: automatic and warm

Cost to build: roughly two weeks more, an estimate. Parts: automatic `up` and `down` under the
operator's budget (§ 7.5); a baked image; object-store sccache; mirror seeding (§ 4.7);
out-of-tree path dependencies and private git databases (§ 4.4); the `cargo run` split and
`--fetch auto` (§ 5.2, § 5.4); mirrors at the workstation's path (§ 5.7); moving a worktree by
measured load (§ 3.2). Delivers: a host in under a minute, warm dependency caches across
nights, most repositories eligible.

### 9.5 How success is measured, with slot's own job log

| measure | source | target during host hours |
|---|---|---|
| local queue wait p90 | local `jobs.jsonl`, `started_ms - enqueued_ms` | from about 30 min to under 5 min |
| overhead per remote job | client wall time minus host enqueue-to-end; from phase 1 `sync_ms` and `fetch_ms` | median under 2 s, p90 under 5 s for warm worktrees |
| share of rust CPU-seconds run remote | `cpu_s` in the copied host log against the local log | at least half |
| workstation CPU and I/O pressure | the census (design.md § 11) | CPU pressure under 20 %, I/O pressure under 15 % |
| workstation disk | free space; local target directories of placed worktrees go idle and are evicted (design.md § 18) | above the eviction mark without manual cleaning |
| cold starts | first job per workspace per host life, run time against the same command's warm p90 | falls with seeding and the object-store cache in phase 3 |
| fallbacks, reruns and lost hosts | counts of fallback lines and `rerun_of` | under 1 % of remote jobs |
| wrong results | a remote test result that a local run of the same tree contradicts | none; each one is a defect with a story |
| cost | price per hour x hours, per night; per remote CPU-hour | within the cap; compared with the queue hours saved |
| credentials crossing | the allowlist tests; a spot check of a remote job's environment and of the mirror | none |

## 10. Open questions and risks

Open questions:

1. **Uplink and round trip** to the nearest region: they set the first sync per worktree (7 s
   to a minute, § 4.6) and every fetch.
2. **Session-manager ssh**: setup time, throughput, and whether one control connection holds
   for hours with 20 jobs on it (§ 8.1).
3. **Which repositories pass the pre-flight**, and what share of the build hours they carry. If
   the heavy ones fail it, the gap fixes of phase 3 move forward.
4. **Seeding across commits** (§ 4.5) and **sccache across workspace ids** (§ 5.6): both decide
   how cold a new worktree starts; the first also whether results are right.
5. **Later gap fixes**: whether cargo uses a locked revision from a synced git database without
   fetching (§ 4.4); whether a mount namespace per job is acceptable on the host (§ 5.7).
6. **Budget, authority and account**: who approves the daily cap and the hours; whether
   automatic start is wanted; which account, network and identity the pool uses; the instance
   role's policy (§ 8.3) and the vCPU quota (§ 8.7).
7. **Trust in fetched binaries**: is "the host is in the workstation's trust domain" acceptable,
   or is a reproducibility check wanted?
8. **Spot interruption rate** of the chosen types; **arm64 hosts** for jobs that fetch nothing.

Risks:

1. **A wrong result that looks right.** Three paths: a seed from another commit (§ 4.5), clock
   skew (§ 4.5), a stale local binary run after a remote build (§ 5.1). Each has a check; none
   is closed before phase 0 has run.
2. **Cold every night.** Target directories end with the host. The first build of each worktree
   is cold (160.7 s on 4 vCPUs in design.md § 16; on 64 not measured). Seeding helps from a
   repository's second worktree on, if § 4.5 allows it.
3. **The host leaves with a full queue behind it.** At the deadline, at a spot reclaim or when
   the link drops, its worktrees come back to stale or evicted local target directories at once
   (§ 3.5). The first hour after a host looks like today.
4. **Overspending** if teardown fails: § 8.7. The residual is the number of instances.
5. **The shared control connection** is one point of failure for every job on a host, and
   sshd's session limit caps the jobs on it (§ 3.5, § 7.3 step 6).
6. **Different results on host and workstation**: other CPU, other kernel, missing workstation
   configuration, rsync's reading of `.gitignore`. A test that passes on one side only is a
   defect of the image or the sync.
7. **Secrets in the tree** (§ 8.4): the deny list is best effort; the opt-in is the control.
8. **`.slot.toml` and older clients.** `RepoConfig` denies unknown fields (`repo_config.rs:25`),
   and a client that cannot parse the file ignores all of it (`client.rs:313-316`), its `local`
   list included. This design adds no key to `.slot.toml`.

## 11. Review notes

Review of 2026-11-23 against the tree at 0.30.0: what changed, and why.

- § 1: each operator question has its own subsection with the command and the traps, to act on.
- § 2.1, § 2.3: only a failed sync falls back; a job lost before its start exits 3 (was: falls back).
- § 3.2: 10 and 30 minutes are the workstation's run limit, not the code's default.
- Citations fixed: `build.rs`, `server/mod.rs` context, `shell.rs` plan, `cache.rs` seed; both env tests.
- § 2.3 added from code: `SLOT_HOST` sends every call; the shim's absolute path; no keepalive.
- § 3.2: per-job spill-over replaced by per-worktree placement with a fixed number per host.
- § 3.2: opt-in moved from `.slot.toml` to the daemon's state; sessions write worktree files.
- § 3.5: failure modes as a table, today beside proposed, including unstarted jobs and reload.
- § 3.4: the health poll copies the host's log, so a reclaimed host loses seconds of it.
- § 4.5 new: modification times, clock skew, and seeding across commits as a correctness risk.
- § 4.4 extended: rsync's `.gitignore` rules, nested `.git`, symlinks, path values, Go modules.
- § 5.1, § 5.3: the stale local binary and the fetch-only-after-exit-0 gap named, with fixes.
- § 5.6 names the open cause of poor sccache hits; § 5.7 new: three options for identical paths.
- § 6: `install`, `doc --open`, `bench`, `publish` added; Go and node each get a paragraph.
- § 7.3: NVMe for all growing directories, sshd limits, metadata firewall, in-host watchdogs.
- § 8.3, § 8.4, § 8.6: role reachable by jobs, values not inspected, returned files validated.
- § 8.7 new: four teardown and cost layers that work with the workstation off.
- § 9: phase 1 is one hand-started host with placement; lifecycle is phase 2; warm is phase 3.
- § 9.1: pre-flight of file list and environment, link check first, a test per unverified claim.
- Prices, sizes and provider behaviour labelled; transfer arithmetic rechecked, unchanged.
- References to `docs/design.md` written `design.md § n`; the old numbering rule was ambiguous.
