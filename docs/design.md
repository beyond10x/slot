# slot: shared build slots and remote build server for Rust, Go and other toolchains — design

Status: 2026-11-12. Phase 0 (local daemon) and the remote transport are implemented in this
repository. Sections 8 and 9 cover toolchains and what was verified. Where the implementation
differs from the first draft, the text below is updated and the difference is listed in §10.

## 1. Problem, from measurement

The numbers come from 38 days of coding-agent session logs on one 20-core developer workstation
shared by many concurrent agent sessions, each working in its own git worktree.

| fact | value |
|---|---|
| Rust build-class calls (`cargo test`, `clippy`, `build`, wrappers) | 22,445 in 38 days |
| `cargo test` duration median / p90 | 9 s / 109 s |
| Hours with at least one build running | 303 h |
| Share of that time with 2+ / 3+ builds running | 46.6 % / 24.0 % |
| Peak concurrent builds | 12, from 4 sessions |
| Same command on the same package, p90 / median (median over keys) | 5.2 |
| Calls serialised behind a `flock` slot file | 1,648; median 80 s vs 9 s without |
| Calls that hand-cap `CARGO_BUILD_JOBS` | 1,938 |
| Calls that switch sccache off (`RUSTC_WRAPPER=`) | 663 |
| sccache Rust hit rate / non-cacheable calls | 65.0 % / 1,036,747 |

What the numbers say:

1. **Contention, not compile speed, sets the tail.** The same command on the same package varies
   5.2x between median and p90. Sessions work around it with three uncoordinated tools: `flock`
   slot files, per-call `CARGO_BUILD_JOBS`, and turning sccache off. None of them sees the other
   sessions, so 12 concurrent builds on 20 cores still happen.
2. **Disk is the second constraint.** Every worktree has its own target directory, and the
   workstation's disk fills.
3. **sccache already covers dependency crates.** It cannot cover workspace crates built
   incrementally, linking, or test execution — the 1,036,747 non-cacheable calls. A distributed
   compile cache alone (sccache-dist) would leave those on the workstation.

Design consequence: move **whole cargo invocations** (compile, link and test run) to a machine with
one scheduler that owns all CPU slots and all target directories.

## 2. Goals and non-goals

Goals
- G1. One global CPU budget for every Rust build from every session; no hand-set `-j`, no `flock`.
- G2. Target directories and the sccache store leave the workstation disk.
- G3. Agent-facing change is one command prefix (`slot cargo …`), output and exit code identical to
  local cargo.
- G4. Overhead per call under 1 s for a warm worktree (median `cargo test` is 9 s).
- G5. If the server is unreachable, the call runs locally and says so on one stderr line.

- G6. Other toolchains (Go first) plug in without changing client, server or UI (§8).

Non-goals
- rust-analyzer / IDE `cargo check` — stays local.
- Bazel/Buck2 remote execution. Bazel is installed, but adopting it means rewriting every build.
- Multi-user service. One user, one trust domain.

## 3. Architecture

```
workstation                                  build server
-----------                                  ------------
session ── slot cargo test -p x                slot serve (daemon, systemd)
             │ 1. rsync worktree  ──ssh──►    /srv/slot/ws/<ws-id>/        (source mirror)
             │ 2. run request     ──ssh──►    queue ──► jobserver (N tokens)
             │                                   cargo test -p x
             │                                   CARGO_TARGET_DIR=/srv/slot/target/<ws-id>
             │                                   RUSTC_WRAPPER=sccache (server-local store)
             │ 3. stream stdout/stderr ◄──    systemd-run --scope (MemoryMax per job)
             └ 4. exit code         ◄──      job log /srv/slot/log/jobs.jsonl
```

### 3.1 Client: `slot`

A single static binary on `PATH` (Go; see §10).

- `slot [--local] [--] <command…>`; `--fetch` is not implemented yet.
- Workspace id: `sha256(hostname + git toplevel path)[:16]`. A worktree under `trees/` gets its own
  id, so parallel worktrees never share a source mirror.
- Sync: `rsync -a --delete --filter=':- .gitignore' --exclude=/target/` over a persistent ssh
  `ControlMaster` socket. Uncommitted changes are included; that is the agent's working state.
  Reruns are frequent (in the logs behind §1: 26,443 same-session reruns, median gap 108 s), so
  the delta per sync is a few changed files.
- Env passthrough: allowlist only — `CARGO_*` except `CARGO_TARGET_DIR` and `CARGO_BUILD_JOBS`,
  `RUSTFLAGS`, `RUSTDOCFLAGS`, `RUST_BACKTRACE`, `RUST_LOG`, `RUST_TEST_THREADS`,
  `NEXTEST_*`, `INSTA_*`, plus names listed in the repo's `.slot.toml`. No secrets, no `HOME` contents.
- Output: stdout and stderr streamed separately; paths under the server mirror are rewritten to the
  local worktree path so `file:line` in errors stays clickable.
- `--fetch target/debug/foo` copies named artifacts back after success (for `cargo build` of a
  binary that is then run locally).
- Fallback (G5): connect timeout 2 s, then run locally with
  `slot: server unreachable, ran locally` on stderr.

### 3.2 Server: `slot serve`

- **Queue and jobserver.** The daemon creates one GNU-make jobserver as a named FIFO with N tokens
  (N = cores) and starts every cargo with `CARGO_MAKEFLAGS`/`MAKEFLAGS=--jobserver-auth=fifo:…`.
  cargo and rustc then share one token pool across all jobs, which replaces every `flock` and
  `CARGO_BUILD_JOBS` in use today. *To verify in the prototype:* that cargo 1.99 honours a
  `fifo:` jobserver passed this way and does not create its own.
- **Test threads.** The jobserver does not limit test-binary threads. The daemon sets
  `RUST_TEST_THREADS` / nextest `--test-threads` to `max(2, free_tokens)` at job start.
- **Memory.** Each job runs in `systemd-run --scope -p MemoryMax=<limit>`; an OOM kills that job
  with a clear exit message, not the server.
- **Target directories.** `/srv/slot/target/<ws-id>`. A new workspace whose repo already has a target
  dir is seeded by a reflink copy (`cp --reflink=always`, XFS or btrfs) of the most recently used
  target dir of the same repo, so a fresh worktree does not start cold.
- **Eviction.** Disk budget per filesystem; when exceeded, delete whole target dirs by last-use time.
  Never touch a target dir with a running job (lock file held by the job).
- **sccache.** Server-local disk store, size set from the disk budget. Same `[profile.dev]`
  (`debug = "line-tables-only"`) and lld linker as the workstation's `~/.cargo/config.toml`.
- **Toolchains.** rustup on the server; `rust-toolchain.toml` in the repo selects the toolchain;
  missing toolchains install on first use (one-time delay logged).
- **Job log.** One JSONL line per job: ws-id, repo, argv, enqueue, start, end, exit code, peak RSS,
  tokens held. This replaces mining session logs for the numbers in section 1.

### 3.2a Same-path mirrors

`SLOT_REMOTE_SAME_PATH=1` on the client mirrors the tree at the client's absolute toplevel instead
of `<ws-root>/<id>`, so absolute paths into the tree and the compiler's working directory are the
same on both machines; it assumes the server's account has the same home. The request carries
`workspace.same_path`; the daemon (`slot serve --same-path-root DIR`, repeatable) runs the job
there only below such a DIR, and `slot shell --same-path-root DIR` admits the sync, fetches and
`slot __siblings <ws-root> <id> <key> <tree>` below it. The sibling step makes `<ws-root>/<id>` a
symbolic link to the tree, so sibling seeding by id and the busy set of eviction keep working;
the sync names a sibling as the absolute `--copy-dest=<ws-root>/<id>`. Eviction removes the link,
not the tree. The sandbox binds the tree writable at its path (`--sandbox-fixed-paths` is not
combined with it).

A same-path job runs as on a CI runner (`same_path.rs`). The tree sync keeps a linked worktree's
`.git` file (`--exclude=/.git/`); the git directories (`--git-common-dir`, and `--git-dir` when
outside it) follow in syncs of their own at their paths, `*.lock` excluded, one at a time per
directory and host. A working directory the tree sync leaves out (under a toolchain exclude or
`git check-ignore`) is synced on its own with the `.gitignore` filter and `--exclude=/target/`.
Outside any git checkout the working directory is the tree, unless it is the same-path root or
above it, lies directly under it, or holds a repository within two levels
(`same_path::tree_refusal`, checked before any sync; a folder that cannot be read is refused):
the job ends with exit 3 and nothing is synced. A folder of several repositories was once synced
whole, with every git-ignored file in it. The request carries
`workspace.target_dir`: `CARGO_TARGET_DIR` of the client, else `cargo metadata`'s
`target_directory` for the command's manifest, else `target` beside the nearest lock file; empty
for a command that is not cargo. The daemon admits it only below a `--same-path-root`, binds it
writable in the sandbox, records it as the job's target directory (which eviction reads) and
names it in the `start` frame, so products are fetched to the same path.

What the job's environment carries of it is a second question, because a compile cache keys
every `CARGO_*` variable of the compiler's environment (sccache: all but `CARGO_MAKEFLAGS`,
`CARGO_BUILD_JOBS`, `CARGO_REGISTRIES_*`, `CARGO_ENCODED_RUSTFLAGS`). The request carries
`workspace.target_default`: true when the directory is the one cargo resolves by itself, that
is `target` in `cargo metadata`'s `workspace_root` with no `CARGO_TARGET_DIR` of the caller
(`same_path::target_dir`, the one place that decides the directory). The daemon then exports no
`CARGO_TARGET_DIR` and removes its own; cargo finds the same directory in the tree. Otherwise
(the caller's variable, a `build.target-dir` of a cargo configuration, `CARGO_BUILD_TARGET_DIR`,
or a client older than the field) it sets `CARGO_TARGET_DIR` to the directory, as before: a
configuration in the caller's home does not exist on the server. So a cargo job, cargo inside a
generic job (which never got the variable) and the jobs of a second tree of the repository
carry the same `CARGO_*` variables. For the same reason a cargo job gets no `CARGO_NET_OFFLINE`
after its fetch (`cargo_fetch.rs`). Not changed: a mirror below the workspace root gets
`CARGO_TARGET_DIR=<cache-root>/target/<id>`, one value per tree, and a sandbox with fixed paths
its own `/work/target`. A server whose own cargo configuration or environment
(`build.target-dir`, `CARGO_BUILD_TARGET_DIR`) names a target directory would move a default
build away from the recorded directory.

### 3.3 Transport and access

- ssh with a dedicated key whose `authorized_keys` entry has
  `command="/path/to/slot shell",restrict`. `slot shell` accepts only the sync and fetch rsync
  steps inside the workspace and cache roots, `slot proxy` and `slot cancel <id>`.
- Network path: LAN or a private overlay (WireGuard/Tailscale). Not exposed publicly.

## 4. Agent integration

- Opt-in per repository: `.slot.toml` at the repo root, plus one `AGENTS.md` line telling agents to use
  `slot cargo …` and that `--local` exists.
- `Taskfile.yml` targets that call cargo use `{{.CARGO}}` with `CARGO: slot cargo` as default, so
  `task check` moves with one variable.
- No `cargo` shim on `PATH` in phase 1: a silent shim would also catch rust-analyzer and tools that
  run cargo for metadata.

## 5. Risks and open questions

1. **Tests that need local services** (docker, databases, a kind cluster, ports on localhost) fail
   remotely. Mitigation: `.slot.toml` lists test filters that run `--local`; the first phase measures
   which ones fail. Not measured yet.
2. **Private git dependencies.** The server needs read access to the git hosts in `Cargo.lock`.
   Options: ssh agent forwarding for `sync` only, or a read-only deploy key on the server.
3. **Build scripts and tests run arbitrary code** on the server. Accepted: same trust domain as the
   workstation. The server holds no credentials beyond the deploy key.
4. **Server sizing is unknown.** Section 1 has busy time and concurrency, not CPU-seconds per call.
   Phase 0 measures core-hours before any hardware is chosen.
5. **Network latency** decides G4. A remote host in another data centre adds round trips to every
   sync; phase 1 measures overhead against G4.

## 6. Rollout

- **Phase 0 — scheduler on the workstation, no new hardware.** Run `slot serve` locally with
  `slot` skipping the rsync step. Delivers G1 (one token pool for 20 cores) immediately and records
  core-hours, peak RSS and queue wait per job. Exit criterion: two weeks of job log; p90/median per
  key compared with 5.2 today.
- **Phase 1 — one repo remote.** The repository with the most build hours goes to the server first.
  Exit criterion: overhead under 1 s at median; list of tests that need `--local`.
- **Phase 2 — all Rust repos,** in order of build hours. Remove the `flock` slot files and
  `CARGO_BUILD_JOBS` from scripts as each repo moves.
- **Phase 3 — workstation cleanup.** Delete local target dirs of migrated repos; shrink the local
  sccache store.

## 7. Alternatives considered

| option | covers | does not cover | verdict |
|---|---|---|---|
| sccache-dist | dependency compiles | incremental workspace crates, linking, tests (1,036,747 non-cacheable calls), disk | rejected as sole answer |
| Bigger workstation | CPU | uncoordinated sessions still oversubscribe; disk | rejected |
| Bazel / Buck2 RBE | everything | requires rewriting all builds | rejected |
| Local jobserver only (phase 0) | contention | disk, CPU ceiling of 20 cores | first step, not the end state |
| Remote whole-command (this design) | contention, disk, CPU | rust-analyzer, tests needing local services | chosen |

## 8. Toolchains

### 8.1 The extension point

Everything toolchain-specific lives in `crates/slot/src/toolchain.rs`. A toolchain is one type:

```rust
pub trait Toolchain: Send + Sync {
    fn name(&self) -> &'static str;                         // label in UI, job log, colour key
    fn matches(&self, argv: &[String]) -> bool;             // does this command belong to me?
    fn tokens(&self) -> usize;                              // slots the server takes before starting
    fn env(&self, c: &Context, env: Vec<String>) -> Vec<String>; // jobserver, caches, parallelism caps
    fn client_env(&self, name: &str) -> bool;               // which client variables cross to a remote server
    fn sync_excludes(&self) -> &'static [&'static str];     // rsync excludes (build output dirs)
}
```

`registry()` is checked in order; `Generic` matches anything and stays last. Client (`slot`), server
(`slot serve`) and the UI (colour, legend, cores-by-toolchain chart) read the registry or the `toolchain`
field of a job, so adding a toolchain touches no other package.

### 8.2 How a toolchain shares the CPU budget

The pool is one GNU-make jobserver FIFO with N tokens. Every job holds `Tokens()` tokens for its
whole run. What the job does with them depends on whether the tool speaks the jobserver protocol:

| kind | examples | Tokens() | Env() |
|---|---|---|---|
| jobserver client | cargo, rustc, GNU make 4.4+ (fifo auth); others to be checked one by one | 1 (the implicit token) | `CARGO_MAKEFLAGS`/`MAKEFLAGS=-j --jobserver-auth=fifo:…`; the tool draws more tokens from the FIFO as it parallelises |
| own parallelism flag | go (`-p`), gradle (`--max-workers`), bazel (`--jobs`), pytest-xdist (`-n`) | fixed K (flag `--go-tokens`, default 4) | sets the tool's own limit to K, so it never exceeds what it holds |
| wrapper | task, make targets, just | 1 | passes the jobserver down; jobserver clients inside share the pool; non-clients inside are not limited |

Rules every toolchain follows:
- Client values of `MAKEFLAGS`, `MFLAGS`, `CARGO_MAKEFLAGS`, `CARGO_BUILD_JOBS` are dropped.
- Shared caches are per toolchain under `--cache-root` in remote mode (`target/<ws-id>`,
  `go-build`); local mode leaves the user's own cache settings alone.
- `client_env` is an allowlist. Credentials (`*_TOKEN`, `AWS_*`, `HOME`) never cross
  (`client_env_is_an_allowlist`).

### 8.3 Go (implemented)

- `matches`: `go`, `gotestsum`, `golangci-lint`.
- Holds `--go-tokens` slots (default 4) and sets `GOFLAGS=… -p=K`. Median `go test` call was 3 s
  (§1), so a fixed K wastes little; a large Go job is capped at K instead of taking all cores.
- Remote: `GOCACHE=<cache-root>/go-build`, shared across workspaces (the Go build cache is safe for
  concurrent use). `GOMODCACHE` stays the server default; private modules need `GOPRIVATE` and git
  credentials on the server (same as §5 item 2).
- Not limited: `t.Parallel()` tests inside one package run up to `GOMAXPROCS`. Setting
  `GOMAXPROCS=K` would cap that, but changes test behaviour; left off until the job log shows Go
  jobs using far more cores than K.

### 8.4 Adding the next toolchain — checklist

1. New type in `crates/slot/src/toolchain.rs`; add it to `registry()` before `Generic`.
2. Decide its kind from the table in 8.2. If the tool reads `MAKEFLAGS`, it is a jobserver client.
3. `env`: cap parallelism to `c.held`; point caches under `c.cache_dir` when it is set.
4. `client_env`: the tool's own variable prefix and nothing else.
5. `sync_excludes`: the tool's output directories (`build/`, `node_modules/`, `.gradle/`).
6. Tests in the same file: `picks_toolchain_by_command` cases, an `env` test, extend
   `client_env_is_an_allowlist`.
7. If the tool runs work in a daemon outside the job's process tree (sccache, gradle daemon,
   bazel server), add its process name to `COMPILE_SERVERS` in `crates/slot/src/server/mod.rs`. CPU is
   then attributed through the `SLOT_JOB` variable, which only works if the daemon passes the
   client's environment to its workers (sccache does, §9). If it does not, the job log
   undercounts that toolchain's CPU and the UI shows it under "host CPU" only.
8. Optional: a colour in `BASE_COLORS` in `ui/src/lib/colors.ts`; unknown names get a
   hashed colour automatically.

Candidates seen in the session logs (§1 counts Rust and Go only): `make`/`task` wrappers
(5,383 calls with unresolved language) already run as `Generic`.

## 9. Verified (2026-11-12, Linux x86_64, 20 cores, cargo 1.99.1, Go 1.27)

These measurements were taken with the first prototype of the daemon, written in Go, which
implemented the same protocol, jobserver and environment rules as the Rust daemon. The test names
below are the Rust tests that cover the same behaviour.

Test crate: `tokio` (full), `serde_json`, `syn` (full), 32 packages, `cargo build --offline`,
sccache off unless stated. rustc processes counted every 0.1 s by their `CARGO_TARGET_DIR`.

| check | result |
|---|---|
| cargo 1.99.1 honours `CARGO_MAKEFLAGS=-j --jobserver-auth=fifo:…`, pool of 2 | max 2 concurrent rustc, 2 runs; 12.8 s and 11.1 s |
| same, pool of 8 | max 8 concurrent rustc; 7.0 s |
| cargo with an inherited jobserver ignores `CARGO_BUILD_JOBS` from the client | server drops it anyway (`jobserver_env_reaches_command_and_client_jobs_are_dropped`) |
| sccache CPU attribution through `SLOT_JOB` (`SCCACHE_RECACHE=1`) | job CPU 14.1 s, of which 12.2 s found in sccache-server children |
| remote mode via a local stand-in for ssh (`rsync -e`, `slot proxy`) | build ran in `<ws-root>/<id>`, target in `<cache-root>/target/<id>`, `target/` and `.git` not synced, compiler paths rewritten to the client repo |
| client disconnect | job's process group killed, token returned (`client_disconnect_kills_job_and_returns_token`) |

Bug found during verification: the Go prototype of the daemon released a job's tokens after the idle
reconcile ran, so the pool grew by one per idle period (log line `restored 1 lost tokens`; a pool
of 2 ran 3 rustc). Fixed by releasing before reconcile; covered by
`client_disconnect_kills_job_and_returns_token`.

Second bug: a go job asks for 4 tokens; on a pool of 2 it was granted 2 but released 4 (pool of 2
showed 4 free after a mixed run). Fixed: a job holds `min(Tokens(), pool size)`, go's `-p` follows
what is held, and reconcile now also removes a surplus. Covered by `request_capped_at_pool_size`,
`go_job_holds_at_most_the_pool` and `reconcile_restores_and_removes`.

Not verified: real ssh to a second machine, `--memory-max`, behaviour with 20+ concurrent jobs.

## 10. Differences from the first draft, and not built yet

Differences:
- One binary: `slot serve` is the daemon, `slot proxy` the remote bridge, `slot <command…>` the
  client. The first prototype was two Go binaries; the port to Rust follows the organization's
  rule that running code is Rust, and the web UI is Vue 3 embedded at compile time.
- The env allowlist comes from `Toolchain::client_env` plus `SLOT_ENV` (comma-separated names),
  not a `.slot.toml` file.
- Remote jobs start from the server's own environment (PATH, HOME, toolchain installs) with the
  allowlisted client variables on top. Local jobs get the client's full environment.
- Every job's environment carries `SLOT_JOB=<id>` for compile-server CPU attribution (§8.4 item 7).
- After a normal exit the server does not kill the job's process group: a sccache server started
  by that job lives in the group and would die with it. On cancel the group is killed.
- The web UI (live timeline, slot usage, cores per toolchain, live output) was not in the draft.

Built after the first draft, tested against a KVM guest (§ 16): `.slot.toml` with `local`
command prefixes (0.7.0), `--fetch` (0.8.0), reflink seeding and budget eviction of server target
dirs (0.9.0), and the forced command, named `slot shell` instead of a separate `slot-shell`
binary (0.10.0).

Built later on the workstation: disk-budget eviction of local target dirs (0.15.0, § 18).

Not built yet:
- Memory limit per job exists as `--memory-max` (systemd scope) but is off by default and untested.

## 11. Measuring the whole host, and an adaptive pool

A fixed pool of one slot per core is right only when slot runs every build. During adoption it
does not, and even afterwards some build work (test binaries, vitest workers, container builds)
runs outside the jobserver. The daemon therefore measures the whole host, not only its jobs.

### 11.1 Census

Every second the daemon already reads `/proc/*/stat`. `crates/slot/src/census.rs` classifies each
process into a tool family from its `comm` and command line: `rust-compile` (any process with
`--crate-name`, including sccache clients), `rust-build-script`, `rust-test` (binaries under
`target/*/deps/`), `go-compile` (`$GOROOT/pkg/tool/*`), `go-test` (`*.test`), `node-test`
(vitest, jest), `node-typecheck` (tsc, vue-tsc), `node-lint`, `node-build`, `node-e2e`
(Playwright), `link`, `native` (C compilers), `helm`, `container-build`. A process inside a slot
job's tree, or compile-server work attributed to one through `SLOT_JOB`, is *managed*; the rest is
*unmanaged*. Each build process is attributed to the agent session in its environment
(`CLAUDE_CODE_SESSION_ID`, `CODEX_SESSION_ID`).

Alongside it the daemon reads pressure stall information (`/proc/pressure/{cpu,memory,io}`,
`some avg10`) and the one-minute load average. PSI is the signal that matters for interactive
use: it is the share of time at least one runnable task waited for a CPU, which is what a
browser tab feels, where load average only counts queue length.

Per second the census goes into the UI tick and `/metrics` (`slot_build_cores{class,managed}`,
`slot_unmanaged_build_cores`, `slot_pressure_some_avg10{resource}`, `slot_host_cores`). Every
10 s a line with the window's means is appended to `<state-dir>/census.jsonl`: host cores, PSI,
load, slots in use, waiters, cores per class (managed classes suffixed `@slot`), cores per session
(total and unmanaged), and the controller's recommendation.

### 11.2 Controller (observe mode)

`crates/slot/src/policy.rs` recommends a pool size each second:

    recommended = clamp(cores − reserve − ewma(unmanaged build cores) − trim, min, --tokens)

- `reserve` (2 cores) stays free for interactive work.
- `ewma` smooths builds outside slot with weight 0.2 per sample, so a burst of one second does
  not shrink the pool and a sustained unmanaged build does within about 10 s.
- `trim` grows by one slot per second while CPU PSI is above 40 % and shrinks by one while it is
  below 20 %, which catches load the census does not classify.

The recommendation is logged and shown; it is not enforced. Enforcement means the daemon holds
`--tokens − recommended` tokens itself, which removes them from the FIFO without touching running
jobs. It is switched on once `census.jsonl` shows, over at least a day of normal work, that the
recommendation stays within a few slots of what the host could take (PSI near the threshold while
the queue is non-empty) and does not oscillate.

### 11.3 Questions the census is collected to answer

1. Which tool families take the CPU when PSI is high: is it Rust compilation, test binaries,
   linkers, or node tooling? That decides which families slot must confine and which can run
   beside the pool.
2. How much build work stays unmanaged after a session adopts slot (test threads, vitest), and so
   how large the reserve must be.
3. Whether queue order should move from FIFO to shortest-predicted-first, using each command's
   history in `jobs.jsonl` (median CPU seconds and run time per repository and argv prefix), so
   short checks are not stuck behind release builds. Answered by § 25: inside a level a job
   predicted at 5 s or less (90th percentile of its last runs) goes ahead of longer ones, until
   they have waited `--pass-for`; `predicted_ms` in the job log shows how often such a job then
   ran over 30 s.
4. Whether memory, not CPU, is the binding limit for parallel link and test phases.

## 12. Why dependencies compile again and again (measured 2026-11-12)

Question: the live view shows the same dependency crates compiling in job after job. Is that
avoidable work?

Method: a sampler read `/proc` every 0.25 s for 15 minutes (11:36–11:51 CET) and recorded every
`rustc` process with `--crate-name`: crate, `-C metadata`, `--out-dir`, working directory and the
`RUSTC_WRAPPER`, `SLOT_JOB` and session variables. sccache counters were read before and after.
On-disk fingerprints (`target/debug/.fingerprint/<crate>-<hash>/lib-<crate>.json`) were compared
across 36 target directories. Caveat: sccache runs `rustc` itself to list a crate's inputs, so the
sampled count includes those calls as well as real compiles.

Findings:

- 4576 of 5216 sampled `rustc` processes (88 %) ran inside slot jobs, one hour after the
  `cargo`/`go` shim went in.
- sccache in the window: 460 Rust hits, 289 misses, 1343 calls it cannot cache (822 "multiple
  input files", 475 "crate-type": proc macros, binaries and test harnesses, which sccache never
  caches).
- 304 of 4262 sampled compiles (excluding these experiments) ran with `RUSTC_WRAPPER=""`, sccache
  switched off. Both sources had a reason. 116 came from one ess task whose consumer-coverage
  check records the compiler profile and accepts the wrappers only when they are empty; that is
  correct and stays. 39 came from a release build pinned to how the previous release was built,
  after the shared sccache server hung on 2026-11-10 and 2026-11-11. A hung sccache server stalls every build that uses it, so a health
  check (`sccache --show-stats` answering within a few seconds) belongs next to idea 1.
- One worktree's target directory held 11 builds of `reqwest` (3 feature sets, 2 profile hashes)
  and 10 of `syn` (7 feature sets, 2 profile hashes). Feature sets differ because resolver 2
  unifies features over the packages selected by `-p`, so `cargo test -p a` and `cargo build -p b`
  build different variants. Profile hashes differ because some commands set
  `CARGO_PROFILE_DEV_DEBUG=0` and others use the machine default (`line-tables-only`).

Tested and rejected:

- `SCCACHE_BASEDIRS`: building one workspace cold in two directories gave 77 of 88 Rust hits in
  the second directory with and without it. Dependencies already hit across directories; the 11
  misses are the workspace's own crates and build scripts. A dependency that includes a file
  from its build script's output directory does not hit across target directories, nor does a
  crate that depends on it: § 28.
- `resolver.feature-unification = "workspace"` (cargo 1.99.1, stable): cargo parses the key and
  rejects bad values, but five `cargo check -p` runs over ess compiled 128 units with it and 128
  without, and left 5 `syn` variants either way. It takes effect only on nightly.

Ideas, none built yet:

1. Keep sccache on: the rust toolchain could warn in the job output when a job sets
   `RUSTC_WRAPPER` empty while sccache is configured. The warning is built
   (`sccache::cache_warnings`, 0.20.0); it does not ask whether sccache is configured.
2. One profile per worktree: warn when the same target directory sees jobs with different
   `CARGO_PROFILE_*` values, since each one builds a full dependency tree.
3. A workspace-hack crate (`cargo-hakari`) in workspaces with many `-p` selections collapses the
   feature variants. Repository change; measure variants per crate before and after.
4. Coalesce identical concurrent jobs: a second job with the same working directory, argv and
   environment as one already running could attach to its output instead of running again.
   Measured 2026-11-12 and not built: in `jobs.jsonl`, 740 jobs over 6.4 h (toolchain not
   `other`), pairs of jobs from different sessions with the same repository and the same argv
   whose run times overlapped: 0 in the same working directory, 0 in different worktrees of the
   same repository. Method: group by `(repo, argv)`, sort by `started_ms`, count pairs where the
   later starts before the earlier ends and `session` differs. Repeat the count before building
   it.
5. sccache size: the cache is at its 20 GiB limit, so least-recently-used entries are evicted.
   Measure the miss rate after raising it to 40 GiB, against the disk floor of 25 GB free.
6. Per-job cache numbers in the UI: units compiled, sccache hits and misses for each job, so
   repeated work shows up where it happens.
7. Watch the sccache server's own jobserver. On 2026-11-12 a server that had run for 8 days
   had 0 free tokens in its jobserver pipe (FIONREAD on its fd 3) and 1 compiler running, on a
   20-core host: tokens leak until every `sccache rustc` on the machine waits behind one compile,
   at 0 % CPU, which looks like a hung server. Writing 18 bytes into the pipe's write end
   (fd 4) started 7 compilers at once; restarting the server also resets it, but fails the
   compiles in flight. Built in 0.5.0 (`sccache::Watchdog`): every 5 s the daemon samples each
   sccache server of its uid (free tokens by `FIONREAD` on the read end of the one pipe the
   server holds both ends of, compiles = its child processes, clients = `sccache` processes with
   the same `SCCACHE_SERVER_PORT`, CPU ticks of the server process). It writes `pool − running`
   tokens only after 60 s in which every sample had no free token, fewer compiles than the pool,
   more clients than compiles, and the server process averaged under 5 % of a core. The pool is
   the host's CPU count or the largest `free + running` seen, whichever is larger. A server
   started under a CPU quota may have had a smaller pool; writing up to the CPU count then
   allows more parallel compiles inside sccache, which slot's jobserver bounds anyway, since
   every compile belongs to a cargo that holds a slot.

## 13. Headroom (0.3.0)

Need: during a video call the desktop must keep CPU while agents keep building. The operator moves
one slider; nothing else changes.

Two limits, because neither is enough alone:

- **Token limit.** The pool keeps `tokens` tokens; with headroom `h` only
  `max(1, round(tokens × (100 − h) / 100))` circulate. The jobserver reader thread keeps the
  surplus as it comes back (`held_back` grows to `reserve`); it cannot take a token a compiler
  holds, so the limit takes effect over seconds. Raising the limit writes the held tokens back at
  once. Queued requests larger than the new limit shrink to it, so no job waits for a token count
  that can no longer exist. Reconciliation counts `size − held_back` as the expected total.
- **CPU quota.** Tokens only count compilers that read the jobserver. Test binaries, linkers'
  threads and node workers do not. `systemctl --user set-property --runtime slot.service
  CPUQuota=(100 − h) × cores %` caps the whole cgroup subtree at once, enforced by the kernel.
  `--runtime` writes a drop-in under `/run` that a reboot drops; the daemon re-applies the saved value
  on start. An empty `CPUQuota=` clears it at 0 %.
- **The base is the host's online CPUs** (`/sys/devices/system/cpu/online`), both for `cores` in
  the quota and for the default pool size. `std::thread::available_parallelism` reads the
  cgroup's `cpu.max`; 0.3.x used it, and a daemon that re-executed inside its own
  `CPUQuota=1200%` on 20 cores counted 12, sized the pool at 12 and set its next quota from 12
  (`headroom 40%: 7 of 12 tokens, cpu quota 720%`, 2026-11-12). Each upgrade under a headroom
  would have shrunk both again. Fixed in 0.4.1.

Jobs under `--memory-max` run in their own `systemd-run --scope` units outside `slot.service`.
Since 0.5.0 those scopes go into `slot.slice` (`--slice`) and the quota is set on `slot.slice` as
well (checked 2026-11-12: a scope in a slice with `CPUQuota=100%` reads `cpu.max` `100000
100000`). The daemon's unit and the slice each get the full quota; the daemon's own CPU is small,
so builds stay under it. Gap: a daemon not under systemd gets the token limit only. Disk IO is not limited: the user manager delegates only `cpu memory pids` to user units, so
`IOWeight` on `slot.service` has no effect without an `io` delegation drop-in for
`user@.service`.

### 13.1 Call mode (0.5.0)

Need: the operator set 40 % by hand for each video call on 2026-11-12. Call mode does it.

Detection, every 5 s, in `call::probe`:

- `pw-dump` (3 s deadline): a `Stream/Input/Audio` or `Stream/Input/Video` node in state
  `running`, without `stream.capture.sink` or `stream.monitor`, with an `active` link from an
  `Audio/Source*` or `Video/Source*` node whose name does not end in `.monitor`. A stream that is
  open but corked (a browser tab with microphone permission, a call app between calls) is `idle`
  or `suspended`, and a recording of what plays is fed by a sink monitor; neither counts.
  Measured on the operator's desktop with no call on: no capture stream, the microphone and the
  camera nodes `suspended`.
- `/proc/*/fd` of the user's processes for `/dev/video*`, except `pipewire` and `wireplumber`:
  Chromium and Firefox read the camera through V4L2, not PipeWire. One pass took 47 ms.
- Ignored applications (`--call-ignore`): level meters, mixers and patchbays, which capture
  without being a call, and `claude`. A test daemon on the operator's desktop on 2026-11-12 saw
  a running capture `PipeWire ALSA [2.1.322]` with no call on: a Claude Code process recording
  voice input through ALSA. Its application name and binary carry only the version, so a stream
  is matched by the `comm` of the client's `application.process.id` as well, and an ALSA client
  is shown by that name.

State (`call::Tracker`): the first probe that sees a call raises the headroom to the call value
when that is higher, without saving it, so a restart during a call comes back with the
operator's value and the next probe raises it again. 120 s (`--call-off-after`) without any
capture ends the call and puts the saved value back. A headroom set by hand during a call is
saved, stays, and the end of that call changes nothing. The call value is saved in
`limits.json` beside the headroom once set through the API; 0 turns call mode off, and turning
it off during a call puts the earlier value back.

Change control: `PUT /api/limits` changes how the whole machine behaves, so it requires a JSON
content type (a cross-site form post cannot send one without a CORS preflight, which the daemon
does not answer) and an `Origin` that is absent or the daemon's own `http://127.0.0.1:<port>` or
`http://localhost:<port>`.

### 13.2 Headroom of the machine (0.16.0)

Need: the operator means the headroom as headroom of the machine. Measured on a 20-CPU
workstation at 45 %: the machine ran 93 to 99 % busy. `slot.service` used 6 to 7 CPUs, inside
its 11-CPU quota; a docker container a pooled job started 3 to 4 (its cgroup is
`/sys/fs/cgroup/docker/<id>`, outside the unit); sccache servers outside the unit, compiling
for pooled cargo jobs, 1 to 2.7; the desktop session scope 6 to 7. The quota caps only the
unit's cgroup subtree.

A base setting, `pool` (default, everything above) or `machine`, saved in `limits.json` as
`base` (absent = `pool`), set by `slot headroom --base` or `headroom_base` in
`PUT /api/limits`, applied at once without a restart. On the machine base, in the sampler tick
(`server/machine.rs`, pure functions tested against a `tempfile` cgroup tree):

- target `T = cores × (100 − h) / 100`, `h` the headroom in force (the call value during a
  call);
- outside `O = busy − pool`: `busy` from `/proc/stat` (all but idle and iowait, as
  `procstat::system_cpu`), `pool` the `usage_usec` delta of the daemon's cgroup (path from
  `/proc/self/cgroup`) plus `slot.slice` beside the user manager's units, where jobs under
  `--memory-max` run;
- `O` smoothed by an exponential moving average, time constant 30 s;
- quota `Q = clamp(T − O, 2, T)` CPUs (`T` when `T < 2`), set through the same
  `systemctl --user set-property --runtime` only when it differs from the last set value by
  0.5 CPU or more and 10 s passed (`systemctl` is a process: never every tick). A setting from
  the API or call mode applies at once with the current average. The token count follows `h`
  as on the pool base.

Per owner, each tick: `usage_usec` of every child of `user@<uid>.service/app.slice` but the
daemon's unit, every `session-*.scope` under `user-<uid>.slice`, every `docker/<id>` (shown by
its 12-character id) and `system.slice` as one; averaged the same way, top 5 shown. A path
that is missing or unreadable is left out, so a host without docker or without a login session
shows fewer. The cgroup tree is read only while the machine base is in force with a headroom
above 0; switching back to `pool` sets the fixed quota again and drops the averages.

State and tick carry `headroom_base`, `busy_cores`, `target_cores` (0 when not in force),
`outside_cores`, `outside` (`{name, cores}`), `quota_floor` and `over_target` (Q at its floor
and `busy > T`: what runs outside the pool uses the headroom and slot cannot take it back);
the tick also `cpu_quota_percent`. `slot status` and the web header print one line from them.

Limits: the pool is never squeezed below 2 CPUs, so a machine loaded from outside stays over
the target. CPU of children of `user@.service` outside `app.slice` (`session.slice`,
`background.slice`) counts in `O` but is not named.


## 14. Admission by disk and memory (0.5.0)

Need: the workspace rule says no build starts under 25 GB free, and a build that runs out of disk
or memory fails late and takes others with it. Slots count CPU only.

Before a job takes slots, `Server::short_of` checks, in this order:

- **Disk.** `statvfs` of the job's directory and of its `CARGO_TARGET_DIR` (relative to the
  directory when relative), at the nearest existing ancestor. Under `--min-free-disk` (GB,
  default 25, 0 off) the job waits. Disk does not come back by waiting for other jobs, so a job
  alone waits too, until space is freed. A job whose last 5 runs of the same `(repo, argv)`
  (`admission::History`, from `jobs.jsonl`) all had `compile_server_cpu_s` 0 and no
  `target_dir` waits at `--min-free-disk-light` instead (GB, default the value of
  `--min-free-disk`, at most that value; 0 lets it start on any free space). A first run, a job
  without a repository and a job with a compile in its last 5 wait at `--min-free-disk`. The
  reason names the floor: `builds need 40.0 GB` or `this job needs 10.0 GB (no compile in its
  last 5 runs)`. Why: toolchain `other` covers `task check`, whose cargo calls run inside the
  job, so the toolchain cannot tell a build from a job that writes next to nothing; its own
  record can. Risk: a script that starts compiling after 5 runs without writes fills the disk
  down to the light floor before its record shows the compile; the light floor is the hard
  floor no job starts below.
- **Memory.** `MemAvailable` less a reservation must cover the job's expected peak plus
  `--min-free-memory` (GB, default 4, 0 off). The reservation is, for each job that started in the
  last 30 s, its expected peak less its sampled memory: a burst of jobs admitted together would
  otherwise each see the same free memory. Jobs running longer than that are counted by
  `MemAvailable` itself; summing every job's 90th percentile would hold builds back that fit in
  practice. With no other job running the check is skipped, so memory never stalls the queue.

Expected peak (`admission::History`): the 90th percentile of `peak_rss` of the last 20 runs of
the same `(repo, argv)` with at least 3, else of the last 200 runs of the toolchain with at least
5, else 1 GB. It is loaded from `jobs.jsonl` at start (1,152 of 1,241 jobs on 2026-11-12 had a
sample) and updated as jobs end. Toolchain 90th percentiles then: rust 3.9 GiB, other 3.5 GiB,
node-test 1.8 GiB, go 0.8 GiB.

The check repeats every 2 s and once more after the slots are granted: a job that finds memory
gone gives its slots back and waits again. The client gets one `slot: job N waiting: …` line per
kind of reason; the job carries `wait_reason` for `slot status` and the UI.

### 14.1 Admission by target directory

Need: two cargo builds into one target directory do not run side by side. The second prints
"Blocking waiting for file lock on build directory" and sleeps until the first has compiled. With
a slot it holds that slot idle while other jobs wait.

A local cargo job that compiles (`cargo_lock::compiles`: `build`, `check`, `clippy`, `test`,
`nextest` and the like, not `fmt` or `metadata`) passes a third gate after disk and memory,
`Server::target_ready`, before it queues for slots. It waits there while another such job with
the same `target_dir`

- holds slots and has no process yet, or started less than 3 s ago (`TARGET_SETTLE`): cargo
  takes the lock only after it has read its configuration and resolved the workspace;
- runs, and the lock of a profile directory of the target directory is held
  (`cargo_lock::held`);
- is queued, is served before it (level, then id) and waits at the gate or in the queue for
  slots. So the builds of one directory start in queue order, and only the first of them
  stands in the queue.

The signal for "is compiling" is cargo's own lock, not the job's processes. `cargo_lock::held`
opens `.cargo-build-lock` and `.cargo-lock` (older cargo) in `<target>/<profile>` and
`<target>/<triple>/<profile>` and asks for a shared `flock` without waiting
(`File::try_lock_shared`, no `unsafe`), which fails exactly while a process holds the file
exclusively; what it gets it gives back at once. The process sample (`LiveJob.units`, 1 Hz) was
not used: it has gaps while cargo holds the lock with no compiler running (fingerprints, waiting
for a jobserver token), it counts a test that starts a compiler, and it does not see a build in
a sandbox (`slot gate`), whose processes are not in the job's tree. The profile directory a
queued command will use is not known from its command line, so a held lock in any of them
counts.

The gate looks again every second (`TARGET_POLL`). A job in the queue for slots looks too and
leaves the queue when its directory is taken, so it gathers no slot it cannot use; at the claim
of its slots, under the server lock, it sees the claim of another job of the directory. A job
enters the queue with its id as arrival number (`Pool::acquire_arrived`), so after any wait
outside the queue it stands where its age puts it. Nothing is persisted: the state is derived
from the job table and the lock. A lock held by a process outside the pool, or by a script job
(no `target_dir`), holds nobody back; such a build starts and blocks in cargo as before.

## 15. Upgrading while jobs run

### 15.1 Bounded hold (0.5.1)

Up to 0.5.0, `systemctl --user reload slot` stopped admission until no job ran, then re-executed.
On 2026-11-12 the reload to 0.5.0 came at 17:07:12 while one `cargo test` job ran; every
other build on the host queued behind it (up to 25 jobs) until it was stopped and the daemon
re-executed at 17:23:37, 16 min 25 s later.

0.5.1 bounds the hold (`server/upgrade.rs`, a pure state machine tested without a daemon):

- On SIGHUP with the pool idle, re-execute at once.
- Otherwise hold admission for at most `--upgrade-hold` (default 60 s, 0 never holds).
- If a job still runs when the hold ends, open admission, log it, and keep the upgrade pending.
- Hold again when every running job is predicted to end within the hold, or re-execute as soon
  as the pool is idle, whichever comes first. A job's prediction is the 90th percentile of the
  run time of its last 20 runs of the same `(repo, argv)` that ended on their own, with at least
  3 (`admission::History::expected_runtime`). A job with no prediction, or past it, blocks a new
  hold; only idle then triggers the switch.
- Admission stays open at least 10 s between holds, so queued jobs start.

`State.upgrade` carries one line for `slot status` (`upgrade pending: …`). The cost: on a busy
host the switch can wait for an idle moment that never comes; the queue keeps moving meanwhile,
which is the property that matters. 15.2 removes the wait.

### 15.2 Handover re-exec (0.11.0)

Built in 0.11.0 (`server/handover.rs`, `Server::hand_over`, `Server::resume`) as designed below,
with these differences:

- The FIFO is reopened by path (`Pool::adopt`), not passed as a descriptor; the state carries
  size, reserve and `held_back`. The jobserver reader polls with a 100 ms timeout on a
  non-blocking descriptor, so `Pool::pause` stops it between two tokens.
- Pumps poll their pipe with a 100 ms timeout and return the pipe when paused. Client writers
  keep serialized frames in a byte buffer and write what the socket takes; a handover takes the
  unsent bytes (`Conn.pending`), so a client blocked on a full socket costs nothing and no frame
  is cut. Every connection is registered with a duplicate of its socket, which survives `exec`.
- One flag under the server lock, `handing_over`, orders everything: a job's slot claim, its
  `Start` frame, its reaping (`waitpid` after `waitid(WNOWAIT)`) and its finish (log line,
  token release, `Exit` frame) each happen under the lock and check it. A job is in the table
  the handover takes, or it finished before, never half.
- Before the irreversible step the daemon checks the installed binary prints a version of
  0.11.0 or later, pauses the pool, and waits up to 2 s for jobs granted slots to have their
  process; a failure there undoes the pause and falls back to 15.1.
- The new process adopts descriptors by number only after checking `/proc/self/fd/<n>` is a
  socket or a pipe, before it opens anything (`take_inherited` runs first in `slot serve`), then
  closes every other inherited descriptor. An unreadable state file closes all of them and kills
  the process group of every child of the daemon. That and the adoption are the only `unsafe`
  code in slot.
- Running jobs enter the table before the first client is accepted, so the idle reconcile
  cannot take the tokens they hold. Queued jobs keep their id and queue time.
- The headroom of a call in progress is not carried: the new process starts from the saved
  headroom and call mode raises it again within 5 s.
- A second reload during a handover (0.15.0). Up to 0.14.0 a SIGHUP that came while the new
  image started, before its tokio handler existed, met SIGHUP's default action and ended the
  daemon; one that came while the old image handed over was dropped. Now `slot serve` installs a
  flag handler right after `take_inherited`; the old image sets SIGHUP to ignored before it
  writes the state (an ignored disposition survives `exec`, a handler does not) and records a
  SIGHUP that came during the handover as `reload` in the state; the new image hands over again
  when either is set. A SIGHUP in the microseconds between the ignore and the new image's
  handler is lost, never fatal.

Measured by `crates/slot-cli/tests/handover.rs`: 20 running and 20 queued jobs, each printing a
numbered line every 10 ms until the test lets it end, through two reloads, the second right
after the first handover finished: every client got an unbroken run of numbered lines, none
lost or doubled, and its exit code, 40 jobs logged once each, and 20 of 20 tokens in the FIFO at
the end. A second test sends 6 reloads 30 ms apart to 4 running and 4 queued jobs: the daemon
keeps its pid and every client gets its output and exit code. The first version waited 1 s
between the reloads and failed 1 of 5 runs on a loaded machine with one handover logged instead
of two. Up to 0.24.0 a handover dropped the frames still in a client writer's channel when it
asked the writer for its unsent bytes: a run of 14 to 40 lines went missing in the middle of one
client's output in 1 of 6 runs at a load of about 100 on 20 CPUs. The writer now hands those
frames on with its unsent bytes.

The design as written before it was built:

Goal: re-execute at once on reload, with jobs running, and keep them: their processes, slots,
output streams and clients. `exec` keeps the pid, so the job processes stay children of the
daemon and `waitid` on them keeps working in the new image. What is lost on `exec` is every
userspace object: threads (output pumps, the jobserver reader), tokio tasks (client writers),
the in-memory job table and every file descriptor marked `FD_CLOEXEC` (all of ours: Rust opens
with `O_CLOEXEC`).

Sequence in the old process, on SIGHUP:

1. Stop accepting: stop polling the listener; new clients queue in the kernel backlog.
2. Quiesce: tell each pump thread to stop after the frame it is sending, wait for the client
   writer tasks to flush, and stop the jobserver reader. Bytes the job writes meanwhile stay in
   its stdout/stderr pipe (64 KiB each by default); a job that fills it blocks for the moment of
   the switch, nothing is lost. Bounded: if a step does not finish in 2 s, fall back to 15.1.
3. Return pool tokens the daemon itself holds for waiters not yet granted, so only tokens owned by
   jobs are out of the FIFO.
4. Write `state_dir/handover.json` (mode 0600, `"version": 1`):
   - listener fd, jobserver FIFO fd (same FIFO path; it is not recreated);
   - pool: size, limit, reserve, `held_back`;
   - per running job: the `Job` record, pid, `held` tokens, fds of its stdout and stderr read
     ends, fd of its client connection, the client's uid, the tracker's last sample, path
     rewrite pair;
   - per queued job: the `Request` and its client fd (so queued clients no longer resubmit);
   - up to 900 one-second chart samples and the buffered output of running jobs and the last 50 completions.
     Older senders omit these optional fields.
   - headroom, call-mode tracker and upgrade counters.
5. Clear `FD_CLOEXEC` on exactly those fds (`fcntl(F_SETFD, 0)`), then `exec` with
   `SLOT_HANDOVER=<path>` in the environment.

In the new process, before binding anything:

1. Read and delete the file. Unknown `version`, or any fd that is not open or not of the expected
   type (`fstat`: socket, FIFO, pipe), aborts the adoption of that item only.
2. Set `FD_CLOEXEC` back on every adopted fd, so jobs started later do not inherit them.
3. Adopt the listener and the FIFO; build the pool from the recorded counts without writing
   tokens.
4. For each running job: restore the table entry, start pumps on its pipe fds and a writer on its
   client fd, and `waitid` on its pid. A job whose pid is gone (it ended during the switch) is
   reaped by that `waitid` with its real exit code, because it is still our child.
5. Re-queue the queued jobs in their old order.

Failure handling: the old image is gone after `exec`, so the new binary must read every format
version an older release writes, and must start even if the file is unreadable. Fallback for an
unreadable file: kill each listed pid's process group, write the recorded `held` tokens back, and
close the client fds, so their clients report `connection to daemon lost` (exit 3) as today
after a restart. A job's output ring (last 256 KiB shown in the UI) is not carried; the UI shows
output from the switch on.

Tests: a two-binary integration test (old binary from the previous tag, new from the tree) that
reloads with a job printing a counter every 10 ms and checks no line is lost or doubled, the exit
code reaches the client, and the pool returns to `size` free; and a fault test per fallback path.

Out of scope: `systemctl restart`, which ends the process and all its children by design.

## 16. Remote mode against a real server (measured 2026-11-12)

Until 0.6.0, remote mode had run only against a local stand-in for ssh. This section is the
first run against a second machine: a KVM guest on the 20-core workstation, Debian 13 cloud
image, 4 vCPUs, 8 GB RAM, 25 GB ext4 disk, qemu user-mode network with ssh forwarded to a
localhost port, rustc 1.100.0, sccache 0.18.0, slot 0.6.0 from the release archive under the
shipped systemd user unit. The client is slot 0.6.0 on the workstation with `SLOT_SSH` naming a
separate ssh config and the default control-connection options.

Test repositories: slot itself (100 crates) and ess (66 workspace crates, 9,199 tracked files,
132 MB synced, two git dependencies), `cargo build -p ess-cli`.

| step | result |
|---|---|
| first sync of ess (empty mirror) plus `slot cargo --version` | 1.75 s wall |
| same again, nothing changed | 0.29 s |
| one file touched | 0.29 s |
| `cargo build -p ess-cli`, cold, sccache empty, 4 slots | 160.7 s wall, 160.3 s run, 426 s CPU, peak RSS 3.6 GB, target dir 4.0 GB |
| same, nothing changed | 0.61 s wall (cargo 0.14 s) |
| same, one line appended to `ess-cli/src/main.rs` | 4.5 s wall, 1 unit |
| same build on the workstation through its own pool, sccache off, 5 to 12 other jobs in the pool | 308 s run after 124 s in the queue, 372 s CPU |
| slot itself, `cargo build -p b10x-slot-cli`, cold | 31.8 s, 100 units |
| ess in a second clone (new workspace id), sccache warm from the first | 159.8 s, 204 units compiled, 30 sccache hits, 256 misses |
| ess in a third clone, target dir copied from the first before the build | 135.6 s, 31 units compiled (the workspace's own crates) |
| copying the 4.0 GB target dir (7,568 files), `cp -a` on ext4 | 36.2 s |

Output streams line by line (202 lines of `Compiling` 8 s into the cold build), and compiler
paths come back rewritten to the workstation's checkout. Exit codes pass through (`sh -c 'exit
7'` gives 7). `slot cancel <id>` with `SLOT_HOST` set cancels the server job in 0.12 s and the
client exits 130. SIGTERM to the client exits 143 and leaves no process on the server. Killing the
ssh connection mid-job prints `slot: connection to daemon lost`, exits 3, and the server cancels
the job (no `sleep` left on the server two seconds later).

Found against the real server, missed by the ssh stand-in:

1. **Credentials crossed.** `CARGO_REGISTRY_TOKEN` and `CARGO_REGISTRIES_<name>_TOKEN` reached
   the job's environment on the server: the rust toolchain admits every `CARGO_*` name. The
   stand-in ran on the same host with the same environment, so no test looked at what arrived.
   Fixed in 0.7.0: `toolchain::credential` holds back names that look like credentials unless
   `SLOT_ENV` names them.
2. **The disk floor stops every job on a small server.** The 25 GB default of `--min-free-disk`
   (§14) is larger than the whole guest disk, so the first job waited with `22.8 GB free …
   builds need 25.0 GB`. The client showed the reason. The setup steps now say to lower it.
3. **`slot` is not on `PATH` over ssh.** Debian puts `~/.local/bin` on `PATH` in login shells
   only; `ssh host slot proxy` is not one, so `SLOT_REMOTE` must name the full path.
4. **A seeded target dir pays only with reflinks.** sccache gave 30 hits in 286 compiles to a
   second workspace id (not analysed; the different target dir in each rustc command line is the
   likely cause), so a new worktree on the server
   rebuilds nearly everything. Copying the first worktree's target dir cut the build from 159.8 s
   to 135.6 s, but the copy itself took 36.2 s on ext4; the gain is the 24 s only when the copy is
   a reflink (btrfs, xfs). Seeding therefore uses `cp --reflink=always` and is skipped where that
   fails.
5. **A full server disk sent the build out of the pool.** With the guest disk full, the sync
   failed (`mkdir … No space left on device`) and the client fell back to running the command
   directly on the workstation, outside the pool: 100 crates compiled unpooled next to a full
   local queue. From 0.8.0 a remote job that does not start goes to the local daemon instead.
   Later a sync that stalls or runs past its bound counts as a failed one, and a new worktree's
   mirror is seeded from its siblings (`docs/design/remote-pool.md` § 4.7).
6. **The memory floor serializes jobs on a small server.** On the 8 GB guest a cold ess build
   expects 3.6 GB peak, and `--min-free-memory` keeps 4 GB free, so a second job waits until the
   first ends (it runs at once only when it is alone in the pool). Lower the floor on a small
   server.
7. **A deleted mirror under a queued job.** Removing a workspace mirror by hand while a job for it
   was queued left the job without a working directory. Eviction of server target directories skips every workspace
   with a queued or running job for this reason.

Seeding and eviction (`server/cache.rs`), measured on the same guest with the cache root on an
8 GB xfs filesystem (reflinks), `--cache-budget 5`, three git worktrees of ess at one commit:

| worktree | what slot did | result |
|---|---|---|
| first | nothing to seed from | 140 s, 204 units, target dir 4.9 GB |
| second | seeded from the first by reflink in 0.4 s | 1 s, 0 units (sources older than the copied outputs) |
| third | deleted the first (least recently used, 4.9 GB; total 9.7 GB over the 5 GB budget), seeded from the second by reflink in 3.8 s | 77 s, 11 units |

The budget counts each target directory in full, so reflinked copies count once per worktree
although they share blocks: xfs showed 4.2 GB used for two directories recorded as 9.7 GB.

"Sources older than the copied outputs" was also the defect: it held for a source that differed
from the sibling's too, so cargo ran the sibling's build of the old source. Since then the sync
keeps the server's modification times (`--no-times`; a changed file gets the time the sync wrote
it), and after a seed every source file of the new mirror with the sibling's bytes takes the
sibling's time, every other one a time after every file of the copy (`cache::align_sources`).
The second row's 0 units hold only for sources that match the sibling's byte for byte. Not
measured again on the guest.

The forced command (`slot shell`, `shell.rs`) on the same guest, with a second key whose
`authorized_keys` line is `command="…/slot shell --cache-root …",restrict`: a build, a build with
`--fetch target/debug/ess` (410 MB back) and `slot cancel` (client exit 130) all ran through it.
Refused with exit 126: no command, `id`, `bash -c id`, `slot proxy; id`, a fetch of
`.ssh/authorized_keys`, a sync into `.ssh/`, `--rsync-path=sh`, `cat /etc/hostname`. A local port
forward through the key failed with `administratively prohibited`. The rsync client in server mode
sends `-logDtpre.iLsfxCIvu`: the `e` before `.` marks protocol capabilities, so `slot shell`
accepts `e` only there; the first unit tests had refused every sync for it.

## 17. Which toolchains run outside the pool (measured 2026-11-12)

Question: with the `cargo` and `go` PATH shim and the Claude Code hook installed, what still
compiles or tests on the host without slot, and which of it belongs in the pool?

Sources, all on one 20-core workstation with about 15 agent sessions:

- The census (§ 11), 3,980 samples over 11.1 h: core-hours per class, inside a slot job
  (`@slot`) or not.
- A 10 min sampler of every `rustc`, `clang`, `ld.lld`, `zig` and `node` process: CPU time, whether
  its environment has `SLOT_JOB`, and its parent chain.
- The Bash commands of every agent session over 3 days (8,490 build or test commands of the
  tools below), with and without a `slot` prefix. `cargo` (1,584) and `go` (912) are not counted
  as outside: the shim sends their build subcommands through slot since the shim was installed.
- A probe: `env` of a test binary under `slot cargo test` shows `SLOT_JOB` and
  `CARGO_MAKEFLAGS=… --jobserver-auth=fifo:…`, so a tool a test starts inherits the pool.

Census, core-hours outside the pool in 11.1 h (inside in brackets): rust-compile 7.18 (25.81),
node 3.55 (7.25), link 0.39 (0.06), rust-test 0.12 (1.98), go-compile 0.09 (0.44), helm 0.05
(0.04), native 0.05 (0.66), container-build 0.01 (0.00). Peak outside: node 10.6 cores, rust-compile
12.7.

rust-compile outside the pool is the sccache server's compilers: in the 10 min sample every
`rustc` (879 CPU-seconds) had `SLOT_JOB` in its environment and sccache as its parent. sccache
runs them outside the job's process tree, so the census does not count them as `@slot`, but the
job log does (`compile_server_cpu_s`, 12.7 core-hours in the same 11 h). Nothing to change for rust.

Node outside the pool in the same 10 min: 80 CPU-seconds in a container (docker), 62 directly from
agent shells or with no parent; more than 1,000 inside slot jobs.

Per tool, the decision:

| tool | seen in 3 days (with `slot`) | evidence of CPU outside | decision |
|---|---|---|---|
| `ignite build` / `test` / `conformance` | 62 (4) / 126 (8) / – | the LLVM target runs `clang` once per compile and link, which reads no jobserver; the test runner starts one test job per core by default | toolchain `ignite` and PATH shim; the hook when the shim is missing |
| `ignite check`, `emit`, `plan` | 353, 80, 10 | front end only | direct |
| `ignite build --target rust` inside `cargo test` | – | cargo sets `CARGO=/usr/bin/cargo` for test binaries, so the shim is skipped; but the job's `SLOT_JOB` and jobserver are inherited (probe above) | covered |
| vitest, jest, playwright | 529, 17, 164 | node-test 1.05, node-e2e 1.58 core-h outside | hook (unchanged), toolchain `node-test` |
| `tsc`, `vue-tsc`, `eslint` | 119, 87, 120 | node-typecheck 0.38, node-lint 0.16 core-h outside | hook |
| package.json scripts `build`, `typecheck`, `lint`, … | 173 (5) | node-build 0.38 core-h outside | hook, by the script name's words; not shimmed, since `npm`, `pnpm` and `yarn` also start dev servers and watchers |
| `node <script> gate` and the like | 829 node scripts, 12 with `slot` | part of node above | hook when the first or second argument is a build word, and `node --test` |
| `zig build` / `test` | 11 | not on `PATH` here; repositories call it through a `zig.sh` wrapper, which a PATH shim does not see | hook, also for `zig.sh`; census class `zig-compile` |
| `pytest` | 78 (10) | no census class before | hook; census class `python-test` |
| `mvn`, `gradle` | 14 | no census class before | hook for goals that compile or test; census class `jvm-build` |
| `docker build` | 97 in 10 sessions | runs in the container daemon, outside the job's process tree and cgroup; container-build 0.01 core-h | out: slot can neither count nor cap it |
| `helm template`, `lint` | 290, 62 | 0.05 core-h, peak 0.6 cores | out |
| `clang` called directly | 71 | native 0.05 core-h outside | out; `ignite` covers the compiler's own calls |
| `valgrind` | 19 | profiles a program's run time | out |

`ignite test` holds one slot per test job, `--jobs` or 4, and sets `IGNITE_TEST_JOBS` to the slots
held. Ignite 0.16.3 reads that variable as the default of `--jobs`; an older ignite inside slot
uses `available_parallelism`, which in `slot.service` is the CPU quota, not the core count.

Measured after: rerun the census and the 3-day count a week after the release; the hook's refusals
on the same 3 days of commands are in the pull request.

## 18. Disk-budget eviction of local target dirs (0.15.0)

Need: the workstation's `/` fills with target directories of worktrees nobody builds in any more;
§ 14 makes jobs wait for disk but nothing frees it.

- **Which directories.** Only target directories a logged job built into. For each local Rust job
  the daemon resolves the target directory (`--target-dir`, `CARGO_TARGET_DIR`,
  `CARGO_BUILD_TARGET_DIR`, else `target/` beside the workspace root found from
  `--manifest-path` or the working directory) and stores it in the job's `target_dir`. At start
  the map of directory to last build time comes from `jobs.jsonl`.
- **When.** Every 60 s, per filesystem (device id): when free space is under the eviction mark,
  directories go least recently built first until free space plus their sizes reaches the mark
  plus 10 GB. Sizes are measured only until that goal is met.
- **The mark.** The larger of `--evict-below` (GB, default 40) and `--min-free-disk` (§ 14,
  default 25); `--evict-below 0` turns eviction off whatever `--min-free-disk` is. Up to 0.24
  the mark was `--evict-below` alone: with `--min-free-disk 50` and the default 40, a filesystem
  between 40 and 50 GB free held every build for disk and nothing was deleted (up to 38 jobs
  queued for about 12 minutes, measured 2026-11-22). `slot status` prints the mark in force and,
  when `--min-free-disk` raised it, says so in the same line (`under 50.0 GB, raised from 40.0 GB
  to the disk hold's mark`). `slot evict-below <GB>` (`PUT /api/evict-below`) replaces
  `--evict-below` at run time and is kept in `limits.json`; `default` forgets it. The watch reads
  it at every look, and a new mark wakes it at once.
- **At the hold.** A job held for disk wakes the watch. It looks at once, instead of at the next
  minute, when its last look found free space at or above the mark (or there was none yet). A
  look that already found it under the mark is not repeated before the minute: sizing
  directories costs a walk of each, and a dry run would size the same ones every 2 s.
- **Kept.** A directory a queued or running job builds into, one in the checkout such a job runs
  in (the job's working directory is at or below the target directory's parent), one built in
  the last 2 h, and one a process or a unit uses. The 2 h are the idle keep: `slot evict-below
  --idle <duration>` (5 min to 7 days, `evict_idle_s` in `limits.json` and in the `PUT` body)
  replaces them at run time; after a host restart about 100 target directories were under 2 h
  old and eviction could free 8 GB of the wanted 100. A directory whose deletion failed (`worktree
  discard-cache` refuses a tree with a live session lease) is left out for 1 h from the failure:
  before, it was planned first again on every look and eviction freed nothing. Before each deletion the job check repeats
  under the server lock. Up to 0.28 any queued or running job of a repository kept every target
  directory of that repository; with about 24 sessions at work that kept nearly all of them.
- **What counts as a target directory.** Absolute, at least three levels deep, a real directory
  (not a symlink), with a `CACHEDIR.TAG` whose first line is Cargo's signature. Anything else is
  never deleted, whatever the log says.
- **How.** When the directory's parent is a managed worktree (`worktree discard-cache --dry-run
  --json <tree>` exits 0), `worktree discard-cache <tree>` deletes it; otherwise
  `remove_dir_all`. A job that wants a directory while it is being deleted waits with
  `slot: waiting: <dir> is being deleted to free disk space`.
- **Dry run.** `--evict-dry-run` defaulted to true from 0.15.0 to 0.18.0; it defaults to false
  since the safety checks above held on the journal. `<state-dir>/evict.json` with
  `{"dry_run": true|false}` overrides the flag; the daemon reads it at every look (no reload) and
  never writes it, so a provisioning tool owns it. In a dry run under the floor the journal says
  `slot: disk: … free, under …: eviction is off (dry run), would free <n> GB in <k> dir(s)`, then
  `would delete <dir> (<size>, last built … ago)` per directory, when the list changes; `slot
  status` shows the same summary as its `disk:` line.


## 19. Builds in a sandbox: the gate (0.18.0)

Need: agent sessions that run in a sandbox (bubblewrap with user, pid and network namespaces and a
seccomp filter that refuses `socket(AF_UNIX)`) could not reach the daemon's socket or its HTTP
port, so `slot` ran their builds outside the pool. The build must stay in the sandbox: running it
through the daemon would run it on the host, outside the sandbox.

What the sandbox can do: open FIFOs and files in directories bound in at the same path as on the
host. A FIFO is an inode, so a hard link to the pool's FIFO in such a directory is the same
jobserver: a cargo in the sandbox draws and returns the same tokens as every other job.

- **`slot gate --dir <dir>`** runs on the host, beside the daemon. It hard-links
  `<state-dir>/jobserver.fifo` to `<dir>/jobserver.fifo` (and again whenever the daemon recreated
  the FIFO at a full restart), creates `<dir>/requests.fifo` and keeps it open read-write, so it
  never sees end-of-file.
- **Client** (`SLOT_GATE=<dir>`): makes `<id>.reply` and `<id>.hold` FIFOs, opens the hold FIFO
  read-write (it stays that FIFO's only writer), writes the request to `<id>.json` and the id as
  one line to `requests.fifo` (under `PIPE_BUF`, so lines of concurrent clients do not mix). No
  reader on `requests.fifo` (`ENXIO`) means no gate: the command runs in the sandbox outside the
  pool, with one stderr line, as with a daemon that is down.
- **Gate → daemon**: one ordinary socket connection per request, with `argv` =
  `slot hold <dir>/<id>.hold` and `hold_for` = the client's command. The daemon queues, books and
  shows the job by `hold_for` (toolchain, slots, memory estimate, target directory) and starts only
  `argv`. A daemon older than 0.18.0 ignores `hold_for` and still starts only the hold process.
  The gate copies the daemon's frames to `<id>.reply`; when the client's reader is gone (`POLLERR`
  on the reply FIFO) it closes the connection, which cancels the job.
- **`slot hold`**: writes its environment, which is the job's (jobserver, `SLOT_JOB`, the
  toolchain's settings), to `<id>.env`, then reads the hold FIFO to end-of-file and exits with
  the code of its last `exit <code>` line, 130 without one.
- **Client after `start`**: reads `<id>.env`, points `--jobserver-auth` at `<dir>/jobserver.fifo`
  and runs the command as its own child. When it exits, the client writes `exit <code>` and closes
  the hold FIFO, waits up to 5 s for the daemon's `exit` frame and exits with the command's code.
  An `exit` frame while the command runs is a cancel: SIGTERM, SIGKILL after 5 s. A client killed
  outright closes the hold FIFO with it, so the hold process ends and the slots go back.
- **Lost tokens**: a compiler in the sandbox holds the FIFO through the link, and its `/proc` fd
  link names the sandbox's path. `jobserver::holders` therefore compares device and inode, not
  the path, so the 15 s lost-token watch (§ 21) does not write back tokens a sandboxed compiler
  holds.

Not covered: CPU and memory of the sandboxed command are not sampled into the job (the hold
process is idle); the sandbox's own cgroup limits apply. The gate does not authenticate clients:
whoever can write to the directory can queue jobs, which is the sandbox the directory is shared
with.

## 20. The promotion delay as a run-time setting (0.21.0)

A queued `normal` or `low` job that has waited longer than the promotion delay is ordered as
`high` (`server/priority.rs`, `level`), so no level waits forever. The delay was only the option
`slot serve --promote-after`. A reload re-executes the daemon with the old process's arguments
(§ 15.2), so a new value in the unit file took effect only at a full restart, which ends every
job.

- **Where it lives.** `priority.json` in the state directory, beside the rules: an optional
  `promote_after_s`. Present, it wins over `--promote-after`; absent, the option applies, so a
  file written before the setting existed keeps its behaviour. The daemon holds the rules and the
  delay in one table under one lock (`priority::Table`) and writes the whole table on every
  change, so a change of the rules keeps the delay and the reverse.
- **How it changes.** `PUT /api/priority` with `{"promote_after_s": <seconds>}`; `null` removes
  the field, and 0 is a delay of its own (never promote). `slot priority promote-after
  <seconds|default>` sends it. The body form `{"pattern", "level"}` is unchanged, both forms may
  share one body, and a body with neither is refused with 400.
- **When it applies.** The level of every queued job is recomputed from the table on each
  change and once a second (`Server::rerank`), so jobs already queued are ordered by the new
  delay at once. A job promoted under a shorter delay goes back to its own level when the delay
  grows past its wait.
- **Versions.** The file's reader ignores fields it does not know, so a daemon without the
  setting reads a file that has it and orders by its own option. When that older daemon saves a
  rule it writes the file without the field: a downgrade loses the delay, not the rules.

## 21. Lost and surplus tokens while the pool is busy (0.21.1)

A process killed while it holds jobserver tokens (a cancel, a `timeout`, a watcher that kills by
run time) takes them with it. Two repairs existed. The idle reconcile sets the FIFO back when no
job runs and none waits. The token watch of 0.12.0 wrote a shortfall back only while no process
but the daemon had the FIFO open, so that no token could be in a live process.

A busy pool reaches neither state. Measured on one 20-CPU host over 27.9 h: 49 of 49 samples
had 3 to 8 other processes with the FIFO open (a `cargo` waiting on a test binary, a `cargo` whose
children are sccache clients, `rustc`), and four losses of 7, 2, 7 and 8 tokens stayed lost for
16 to 173 minutes; in the longest, 8 of 14 circulating tokens were gone. Later, after a watcher
killed 12 running jobs on a pool of 50, five counts over 8 minutes each found at least 19 to 23
tokens that no live process held while 30 or more jobs waited.

### 21.1 The rule

**The watch never writes back a token a live process may hold.** It counts an upper bound of the
tokens in live processes and writes back only what is missing beyond it.

Every 15 s (`server::TOKEN_WATCH`; `Config::token_watch` in tests) one pass of
`Server::token_pass` reads:

- **unbooked**: the pool size, less the tokens in the FIFO, the tokens held back for the headroom,
  the slots granted to running jobs and the tokens queued requests have gathered (`Pool::missing`;
  one request holds them, the one that stops the walk of § 25, else the first the tokens at hand
  do not cover; `slot status` names them as collected). It is read under the server lock, so a
  job that ends is counted once, and it
  is read before and after the walk over `/proc`; the smaller value counts, so a token given back
  during the walk is not missing.
- **bound**: the sum, over every live process with the FIFO open (`jobserver::holders`, by device
  and inode, § 19), of the most tokens that process can hold (`Holder::may_hold`, below).
- **shortfall**: unbooked less bound, when positive. These are the tokens that neither a job nor a
  live holder accounts for.

The watch writes back the smaller of the shortfalls of two passes in a row, so a token the
daemon's reader thread is booking at one of them is not taken for lost. A lost token is therefore
back after at most two passes and one walk: under 45 s. The walk runs in `spawn_blocking`, and
only when something is unbooked.

### 21.2 What a live holder can hold

A jobserver client holds one token for each unit of work it runs beyond the first. The first
runs on the token the client was started with: for the first process of a job, a slot the daemon
granted to the job. `Holder::may_hold` counts from `/proc/<pid>/stat` of the holder and of every
process whose parent it is:

| holder | units of work | bound |
|---|---|---|
| a build driver (`comm` is `cargo`, `make`, `gmake` or `ninja`) | its direct children that it has not reaped, running or zombie, whatever they are | children − 1 |
| any other process | the larger of its children and its threads | that − 1 |
| either, with more than one thread | | one more |

The reasons, each from a measurement:

- **Children, not compilers.** In 40 samples the unbooked tokens equalled the sum over holders of
  (children − 1) 38 times and were 1 above it twice. A `cargo` whose 2 children were sccache
  clients held 1 token with no compiler anywhere in its subtree (15 observations): the compilers
  run under the compile server. So the bound counts a driver's children whatever they are, and
  "has a live compiler below it" is not the test.
- **Threads for everything but a driver.** A `rustc` with the FIFO open has no child and holds a
  token for each codegen thread after its first. A driver's threads are not counted, because they
  mirror its children: `cargo` processes had 4, 5 and 7 threads with 1, 2 and 4 children.
- **One more for a holder with several threads.** Such a holder reads tokens on a helper thread,
  and a token it has read is not a child or a worker thread yet for a moment; that is the "1
  above" of the 40 samples. A holder with one thread starts the child in the thread that read
  the token.

With that a compiling `cargo` with `n` children counts for `n` tokens and holds `n − 1`; a
`rustc` counts for its thread count; a single-threaded client with `n` children counts for
`n − 1`.

### 21.3 Two cases that are not a loss

1. **A slow live holder.** A process that has held its tokens for minutes and uses no CPU (a
   long compile, a `cargo` blocked on a lock, a build waiting on a test binary) still holds them.
   Neither the age of a holder nor its CPU use enters the count; only the death of the process
   removes its share from the bound.
2. **A holder whose children are compile-server clients.** Its subtree has no compiler, and it
   holds tokens all the same (§ 21.2). The bound comes from the number of children.

### 21.4 Surplus

More tokens than the pool has can be in circulation: a slot released twice, or a token written
back that a live process still held. Whatever live holders hold only adds to the count, so
`Pool::surplus` needs no bound: tokens in the FIFO, held back, granted and gathered, less the
pool size. The same pass takes out the smaller of two such counts in a row (`Pool::remove`): the
tokens in the FIFO at once and, for the rest, the next ones that come back, which the reader
thread keeps (`debt`). Before, only the idle reconcile removed a surplus.

### 21.5 What is shown

- `slot status`: `lost slots: 3 that no job and no live process holds, oldest for 12s`, only while
  the last pass found a shortfall. `State` carries `tokens_unaccounted` and
  `tokens_unaccounted_since_ms`.
- `/metrics`: the gauge `slot_tokens_unaccounted` beside the counter `slot_tokens_restored_total`.
- The journal: `slot: <n> jobserver token(s) were lost (…): wrote them back` and
  `slot: <n> jobserver token(s) more than the pool's <size> …: took them out`.
- The census log: `tokens_restored` with `tokens`, `granted`, `holders` and `bound`;
  `tokens_removed` with `tokens` and `granted`.

### 21.6 Limits

- The bound is above what the holders hold, by one for each holder with several threads. Lost
  tokens within that margin stay out until such a holder exits; the two passes after that write
  them back.
- A process that is not a build driver counts for its threads for as long as it has the FIFO
  open. A long-lived one with many threads (a compile server started inside a job, if it
  keeps the job's jobserver open; not measured) hides that many lost tokens until it exits. A driver is known by its `comm`
  only: a driver under another name counts as any other process, which is the larger bound.
- One walk reads `stat` and every descriptor of every process. Reading the same with `cat` and
  `stat` took 0.19 s for 1,300 processes with 9,700 descriptors on a workstation at load 84. The
  walk runs only while something is unbooked.
- `/proc` is not read in one instant. A holder that takes a token and starts a child while the
  walk is past it is covered by the two readings of the unbooked count, by the one token more
  for a holder with threads, and by the second pass; a client that held more tokens than units of
  work at two passes in a row would get one written back. The surplus rule then takes it out
  once the holders give their tokens back.
- A handover (§ 15.2) does not pass on the tokens still to take out; the new image counts again.

## 22. The run-time limit (0.22.0)

A job that hangs holds its slots until its client goes away. The run-time limit ends a job that
has run longer than a set time, counted from `started_ms`; the queue wait is not part of it
(`run_limit.rs`).

- **Settings.** `limits.json`, beside the headroom: optional `run_limit_s` (absent: no limit, the
  default of a fresh install) and `run_limit_max_s` (the most a job may name; absent: 3600).
  `GET` and `PUT /api/run-limit` read and change them, `slot run-limit <duration|off> [--max
  <duration>]` sends the change. A field left out of the body keeps its value, `run_limit_s` 0
  turns the limit off, and a limit over the maximum is refused with 400, so the maximum is never
  below what every job gets anyway.
- **A job's limit** (`run_limit::job_limit`). Its own `timeout_s` (`slot --timeout`,
  `SLOT_TIMEOUT`; a field of the request that an older daemon ignores), capped at the maximum;
  else the limit.
- **A wrapped `timeout` extends nothing.** The alternative, giving a job that starts with coreutils
  `timeout` that command's time, was dropped: a `timeout 1800` goes in front of every command by
  habit, and the limit then bounds almost no job. Only `slot --timeout` extends a job. The
  daemon still reads the time of a `timeout` that is the first or
  second word of the command (`run_limit::wrapper_timeout`), for one `info` frame at submission
  when it names more than the job's limit: the job ends earlier than its caller wrote, and the
  line names the `slot --timeout` to use.
- **When it is fixed.** At the job's start, from the settings of that moment, and written to the
  job record as `limit_s`. A later `slot run-limit` changes the jobs that start afterwards. A
  setting that reached back would end a job that was started under a promise of more time.
- **Enforcement.** Once a second (`Server::end_overdue`) every running job with `limit_s` whose
  run time reached it is marked `timed_out` and its supervisor is told. The supervisor ends it as
  it ends a cancelled job: SIGTERM to the process group, SIGKILL after 5 s. The job is logged as
  `failed` with exit 124 and `timed_out`, and the client gets one `info` frame and exit 124. A
  job whose client already went away is cancelled, not timed out.
- **Handover.** `limit_s`, `timed_out` and `started_ms` are fields of the job record, which a
  handover passes on whole (§ 15.2), and the request of a queued job carries `timeout_s`. The
  handover format and its `VERSION` are unchanged. The new image's watch counts on from
  `started_ms`; a job that was signalled by the old image and not yet reaped is signalled again.
- **Versions.** A daemon without the limit reads a `limits.json` that has the fields and ignores
  them; when it saves the headroom it writes the file without them, so a downgrade loses the
  limit. A client without `--timeout` and a daemon without the limit talk to each other
  unchanged.

## 23. Build processes per job (0.23.0)

**The problem.** Each `rust` or `other` job holds one slot, and its `cargo` reads one more token
from the FIFO for every compiler it starts (§ 8). The daemon reads the same FIFO for the waiting
jobs, as one reader among the `cargo` processes of the running ones. Measured on one host with a
pool of 50 slots: 7 `cargo` processes read the FIFO against the daemon; one `cargo` had 19 child
processes; 10 jobs ran while 49 waited. 15 tokens written back raised the compilers under `cargo`
from 19 to 34 and the running jobs from 9 to 10. A freed slot goes to the compilers of jobs that
already run, not to the queue.

**No bound per job inside the pool.** The first design set `CARGO_BUILD_JOBS=N` beside the pool's
jobserver. cargo does not read it there (cargo 1.99.1, commit `5f94df478`, paths in the cargo
repository):

- `src/compiler/build_runner/mod.rs:109-116`: `jobs` only sizes the jobserver cargo makes when
  it inherits none (`Client::new(bcx.jobs())` in the `None` arm of `jobserver_from_env()`).
- `src/compiler/job_queue/mod.rs:607-609`: the only bound on units at once is
  `active.len() < tokens.len() + 1`, the tokens in hand; `jobs` is not compared.
- `src/compiler/build_config.rs:79-85`: a `-j` beside an inherited jobserver gets the warning
  "ignoring the `-j` parameter"; the value from `build.jobs` or `CARGO_BUILD_JOBS` is dropped the
  same way, without one.
- The only other reader of `jobs` is `NUM_JOBS` for build scripts
  (`src/compiler/custom_build.rs:374`).

A probe confirmed it: 12 independent empty crates, a wrapper that counts the compiler units alive
at once, outside any pool.

| environment of `cargo build` | most units at once |
|---|---|
| no jobserver, `CARGO_BUILD_JOBS=2` | 2 |
| inherited FIFO with 8 tokens, `CARGO_BUILD_JOBS=2` | 9 |
| inherited FIFO with 8 tokens, no `CARGO_BUILD_JOBS` | 9 |
| inherited FIFO with 8 tokens, `cargo build -j 2` | 9, and the warning |

**The mechanism** (`job_parallel.rs`, `toolchain::own_jobserver`). With the setting at N the
daemon starts a job without the pool's jobserver. After the toolchain made the job's environment,
the four variables of `toolchain::STRIPPED` are taken out again and two are set:
`CARGO_BUILD_JOBS=<P>` and `MAKEFLAGS=-j<P>`, P = max(N, slots the job holds). This happens in the
one place that starts a job (`Server::run_job`), after `Toolchain::env`, so it covers every
toolchain, a remote job (the server's daemon builds its environment there) and a gated job (the
hold process writes that environment to `<id>.env`; `gate::rewrite_auth` finds no
`--jobserver-auth` to rewrite). cargo finds `MAKEFLAGS=-j<P>` with no `--jobserver-auth`
(`jobserver` 0.1.35 `src/lib.rs:254-274`: the first of `CARGO_MAKEFLAGS`, `MAKEFLAGS`, `MFLAGS`
that is set, here an error of kind `NoJobserver`, which cargo takes as "none",
`src/context/mod.rs:325-337`), makes a jobserver of P tokens and hands it to rustc and to build
scripts in `CARGO_MAKEFLAGS`. The job waits for and holds its slots as before. What the
toolchains derive from held slots stays: `GOFLAGS -p`, `GOMAXPROCS`, `IGNITE_TEST_JOBS`, the
test-thread defaults.

- **Settings.** `limits.json`: optional `job_parallel` (absent: off, and the file is then
  byte for byte what a daemon without the setting writes). `GET` and `PUT /api/job-parallel`
  (`{"job_parallel": 4}`, `null` for off; under 1, and a body without the field, are refused with
  400), `slot job-parallel [<N>|off]`, `State.job_parallel`, one header line in `slot status`.
- **When it is fixed.** At the job's start, like the run-time limit (§ 22). A job that runs keeps
  its environment; the setting reaches the jobs that start after it.
- **Handover.** Nothing travels: the new image reads `limits.json` at its start, and a running
  job has its environment already. The handover format and its `VERSION` are unchanged.
- **A nested `slot` call** (a `slot cargo …` inside a job) runs directly with the job's
  environment (`client::nested`), so it makes a jobserver of P of its own.

**`MAKEFLAGS=-j<P>` and build scripts.** The variable stays in the environment of a build
script, because cargo hands its jobserver on in `CARGO_MAKEFLAGS` only
(`src/compiler/custom_build.rs:388` `inherit_jobserver`, `crates/cargo-util/src/process_builder.rs:519`
`configure`, `jobserver` 0.1.35 `src/lib.rs:383-386`). The crates build scripts use do not read it
there:

- `cc` 1.9.0 takes its jobserver from `jobserver::Client::from_env()`
  (`src/parallel/job_token.rs:129-130`), which reads `CARGO_MAKEFLAGS` first: cargo's jobserver.
  It starts no `make`.
- `cmake` 0.1.58 sets `MAKEFLAGS` to the value of `CARGO_MAKEFLAGS` for a Makefile build
  (`src/lib.rs:841-864`): cargo's jobserver. For another generator it passes `--parallel
  $NUM_JOBS` (`:883-887`), and `NUM_JOBS` is P.
- `jobserver::Client::configure_make` sets all three variables (`src/lib.rs:406-412`).

So a `make` started through these stays inside its cargo's P. A build script that starts `make`
itself and sets no `MAKEFLAGS` gets `-j<P>` beside cargo's P; of 331 build scripts in one
registry cache of 2,378 crates, none starts `make` or `ninja` directly. For that reason the
variable is set for every job and not only for a job whose command is `make`: a `make` anywhere
in a job's process tree is bounded, where without the variable it would run one recipe at a
time and a `make -j` in a script would have no bound at all.

**sccache.** sccache 0.18.0 leaves `CARGO_BUILD_JOBS` out of its cache key
(`src/compiler/rust.rs:1580-1586`; in from 0.12.0, absent in 0.11.0), and `MAKEFLAGS` was never
in it (only `CARGO_*` variables are). The setting changes no key. An sccache server always makes
a jobserver of its own, one token per CPU (`src/jobserver.rs:48-55`), whatever the job's
environment says.

**What reads the FIFO's holders.**

- The token watch (§ 21, `Server::token_pass`) acts only while tokens are unplaced. With every
  job on a bound of its own nothing but the daemon reads the FIFO, nothing is unplaced, and each
  pass ends at `missing == 0` with `tokens_unaccounted` 0. A job from before the switch still
  holds the FIFO and is counted as before (`jobserver::Holder::may_hold`).
- The `sccache:` line of `slot status` counts the tokens of each sccache server's private pipe
  (`sccache.rs`), not the pool's.
- `Context::free_tokens`, the default of the test-thread variables when `--test-threads` is 0,
  reads the FIFO's fill at the job's start. With the setting on no compiler takes tokens, so it
  is the slots no job holds.

**Measured through a daemon.** The same 12 crates, one job at a time through a private daemon of
this build with a pool of 8 slots:

| setting | the job's environment | most units at once |
|---|---|---|
| off | the pool's `--jobserver-auth` in `MAKEFLAGS` and `CARGO_MAKEFLAGS` | 8 (its slot and 7 tokens) |
| 4 | `CARGO_BUILD_JOBS=4`, `MAKEFLAGS=-j4` | 4 |
| 2 | `CARGO_BUILD_JOBS=2`, `MAKEFLAGS=-j2` | 2 |
| 2, job with `--tokens 6` | `CARGO_BUILD_JOBS=6`, `MAKEFLAGS=-j6` | 6 |
| 4, `make` with 8 recipes (GNU Make 4.4.1) | `MAKEFLAGS=-j4` | 4 |

Each run compiled the 12 crates (13 units with cargo's probe of the compiler), all 8 tokens were
in the FIFO afterwards and the daemon logged no lost token.

**The cost.** The pool bounds jobs, not compilers: S slots run up to S jobs of one slot and
S × N build processes, where the pool alone allowed S. A job no longer narrows when the pool is
full. The bound is per `cargo` and per `make`: a script that runs two `cargo` commands at once
runs 2 × P. `CPUQuota` of the headroom (§ 13) still caps the CPU time of all jobs together. What
the pool keeps: the number of jobs, the queue order, admission by memory and disk, and every
slot a toolchain holds up front (go, node-test, ignite).

**Not chosen: a FIFO per job that the daemon feeds from the pool.** It would keep "every compiler
holds a pool slot" and still bound a job, and one job's `cargo` and `make` processes would share
one bound. It needs a FIFO and a feeder per job, the daemon cannot see how many tokens a job
wants, and each FIFO has to cross a handover. Open if S × N turns out too wide.

## 24. A hold on the host's busy CPUs (0.24.0)

**The problem.** Slots bound what the pool starts. They do not bound what the host runs.
Measured on one host of 20 CPUs with a pool of 50 slots: the host was 16.7 to 18.5 CPUs busy for
three hours, with 46 runnable threads at one sample, while the queue was empty and 35 slots were
free. Over 473 runs of 31 repeated commands the same command took 0.55 times its median run time
at a CPU pressure under 50 % and 1.19 times over 80 %. About half of the compiler CPU ran in the
compile cache server's own cgroup, outside the `CPUQuota` of the headroom (§ 13), so the quota
does not see it. The host-wide figure does.

**The setting** (`cpu_limit.rs`). `limits.json`: optional `cpu_busy_max` (the mark, in CPUs;
absent: off, and the file is then byte for byte what a daemon without the setting writes) and
`cpu_busy_floor` (jobs that may always run; absent: 4). `GET` and `PUT /api/cpu-limit`, `slot
cpu-limit [<CPUs>|off] [--floor <N>]`, `State.cpu_busy_max` and `State.cpu_busy_floor`, one header
line in `slot status`. `GET /api/cpu-limit` answers the parts of the figure as `pool`,
`compile_cache` and `outside` (CPUs, `null` before the first sample) beside `busy`, the host's
total; `slot cpu-limit` prints them. A mark under 1 or not finite, a floor under 1 and a floor without a mark
are refused. A mark above the host's CPU count is taken and holds nothing. Released switched off.

**The hold** (`cpu_limit::verdict`, `Server::cpu_verdict`). The third check of
`Server::short_of`, after disk and memory, of kind `cpu`. A queued job that could start is held
while both are true:

1. at least `floor` jobs run (a job counts from the claim of its slots);
2. busy + starting + ahead ≥ mark.

- **busy.** The CPU of the pool's own work: `pool + compile cache` of the census sampler (§ 11),
  from the CPU ticks of each process in `/proc` over one interval (`census::Sample::parts`).
  - *pool*: every process in the tree of a running job, and the compilers of a compile cache
    server that carry a job's `SLOT_JOB` (`compile_server_work`), so the CPU a job's compiles
    use inside the cache server counts once, here.
  - *compile cache*: the rest of each compile cache server's tree (`sccache` outside every
    job tree, `census::compile_cache_pids`): the server itself and the compilers it runs for
    builds outside the pool. A server is an `sccache` whose parent is the service manager
    (`systemd`, pid 1); an `sccache` under `cargo` or another build tool is a client, and the
    compiles and links it runs itself (the calls the server does not cache) count as *outside*.
  - *outside*: every other process; shown, not counted.

  The three parts do not overlap and add up to `Sample::host_cores`. The sampler runs once a
  second; the hold reads the mean of its last 10 samples (`cpu_limit::Recent`, the span of one
  census log line) and scans nothing itself. One sample swings with every process that starts or
  ends; the mean of 10 moves at most a tenth of that a second. The sampler's first sample has no
  earlier reading and is left out. A mean whose last sample is older than 5 s is no figure. A
  process that starts and ends inside one interval is in no sample.

  Up to 0.24 the figure was `host_cores`, every process of the host. Measured 2026-11-22 with the
  mark 17 and the floor 6: 7.7 to 10.8 of 16 to 20 busy CPUs were outside the pool and outside
  the compile cache server, and for 11 minutes 78 jobs waited 204 s at the median for that work;
  the limit was switched off. The pool's work alone is what the pool can make room for.
- **starting.** The memory hold's device (§ 14): a job that claimed its slots, or started under
  30 s ago, counts with `expected − shown`, at least 0. `expected` is the median of `cpu_s ÷ run
  time` over the last 20 runs of the same repository and argv that ended on their own and ran a
  second or longer (`admission::History::expected_cores`, at least 3 runs), else 1.0, at most the
  host's CPUs. `shown` is the sum of the job's own one-second samples over the window of the
  mean, which is the job's part of `busy`. Without this term every held job would pass in the
  tick where the mean dips under the mark.
- **ahead.** The expected CPUs of the jobs that are held for CPU, or were let through and have
  not claimed their slots yet, and that the queue serves before this one (level, then id). Held
  jobs so start in the queue's order, and the queue's order itself is unchanged.

The verdict is computed under the server lock, in `short_of` and again in the block that claims
the slots, so of the jobs that pass at the same moment each one counts the claims before it. A
held job holds no slot.

**The bound.** A job that has been held for CPU for 10 minutes (`cpu_limit::HOLD_MAX`, counted
from its first CPU hold) starts: one `info` frame, one line in the daemon's log, and a
`cpu_hold_bound` event in the census log. A job's first CPU hold writes a `cpu_hold` event. The
time is not part of the handover: after a reload a held job's 10 minutes start again.

**No figure.** A daemon younger than 5 s has no mean yet and holds every start past the floor
(`Figure::Pending`); otherwise a reload would start every held job in its first second. After
that, no fresh sample (`Figure::Unknown`) holds nothing.

**Handover.** Nothing travels: the new image reads `limits.json` at its start. The handover
format and its `VERSION` are unchanged.

**Versions.** A daemon without the setting reads a `limits.json` that has the fields and ignores
them; when it saves the headroom it writes the file without them, so a downgrade loses the limit.

**The cost.** With the limit on a queue forms while the host is saturated, also when slots are
free, and a job can wait up to 10 minutes for load that is not the pool's. The expected CPUs of a
job do not include what its compilers use inside a compile cache server (`compile_server_cpu_s`
is not attributed at the start), so `starting` is low for such a job until the mean shows it.

**Not chosen.**

- *`sys_cpu` × CPUs from `/proc/stat` as the figure.* It also counts processes that live under a
  second, but it has no part per job: `shown` is taken from the same per-process ticks as
  `host_cores`, so the two subtract cleanly.
- *One sample as the figure.* One second under the mark would start a job.
- *A second scan of `/proc` per admission check.* The sampler's scan is the one walk a second;
  a queue of held jobs that each look every 2 s would multiply it.
- *The CPU of the `slot.service` and `sccache.service` cgroups as the parts.* It would count
  processes that live under a second, but it reads a layout only systemd gives, it counts the
  daemon's sccache children that serve pool jobs under the compile cache instead of the job, and
  `shown` of a starting job would no longer come from the same ticks as the figure.

## 25. Passing in the queue: whole-pool requests and short jobs (0.25.0)

Two measurements on one 20-CPU host, both on 2026-11-22.

- **A request for every slot stalls the queue.** A job submitted with `--tokens 17`, with 17
  slots in circulation, stood first in the queue for about 13 minutes. It had collected 11 slots;
  5 running jobs held 6, one of them for over an hour. 54 jobs waited behind it, 53 of them for 1
  slot. The reader gave every returning token to the head of the queue, and the head kept it.
  `slot status` read "0 free, 5 running, 54 queued" and named none of the 11. After the job was
  cancelled, 17 jobs ran within minutes.
- **Short jobs carry most of the queue wait.** On the busiest day of the log (17,682 started
  jobs) 72 % of the jobs ran under 5 s and carried 47.4 h of the day's 94.8 h of queue wait. A
  replay with the daemon's own predictor (`admission::History::expected_runtime`, the 90th
  percentile of the last 20 runs of the same repository and argv that ended on their own, with at
  least 3) predicted 44.7 % of the jobs at 5 s or less; they waited 22.6 h and needed 1.8
  slot-hours, and 0.1 % of them then ran over 30 s.

Both answers let a request start before an earlier one, and each needs a bound for the request
it passes. One bound serves both.

**The bound** (`slot serve --pass-for <seconds>`, 300 by default; `Config::pass_for`). A queued
job may be passed by later jobs of its own level only until it has waited this long. From then on
it starts before every job that came after it. The wait is counted from the job's enqueue time
on the daemon's clock (`Config::clock`), as the promotion delay is (§ 20), so a job that left the
queue for memory or its target directory keeps its age. That clock is the wall clock and can step
back; a job stays unpassable once it was (`Pool::set_place` keeps it so, and `Server::place_of`
reads the class the job last had in the pool, so a job that leaves the queue and comes back keeps
it too). `--pass-for 0`: nobody
passes, and the queue is served by level, then arrival, as before. A reload re-executes with the
old arguments (§ 15.2), so a changed value takes effect at a full restart.

**The place** (`jobserver::Place`, `server::priority::place`). Every request carries two flags
beside its level and its arrival:

- `passable`: it has waited less than the bound.
- `short`: it is passable and its predicted run time is at most 5 s (`priority::SHORT_MS`). A job
  with fewer than 3 earlier runs has no prediction and is not short.

The queue's order is level first; inside a level the requests that are no longer passable, then
the short ones, then the other passable ones; arrival last (`WaiterState::rank`). Since waits
grow with arrival, the unpassable requests of a level are its oldest. The daemon sets the place
at enqueue and again once a second (`Server::rerank`), so a request becomes unpassable within a
second of its bound without any other event; the pool sorts its queue on every change.

**Serving the queue** (`Pool::distribute`). The pool serves from the tokens at hand: every
token the queued requests gathered, plus the one the reader just took. It walks the queue in
order and grants each request the tokens at hand cover. A request they do not cover stops the
walk when it is not passable: nobody behind it takes a token, it gathers each token that comes
back and starts when it has all, which is the old behaviour. While it is passable the walk goes
on through its own level, and a later request of that level that fits in what is at hand starts
ahead of it; the walk stops at the first request of a lower level, so passing never crosses
levels. Tokens the headroom lent an `urgent` request (`Pool::lend`) are not at hand: they stay
with it, or go back to the held-back set once it is not `urgent`, so no other work runs beyond
the limit on them. What is left stays
booked with one request (the one that stopped the walk, else the first not covered), so two
requests never each hold part of what one of them needs; with nobody waiting it goes back to the
FIFO. With 4 slots, one running job on 1, a 4-slot request and three 1-slot requests behind it:
the 4-slot request gathers 3, and each 1-slot request takes one of them as it queues.

**The cost of the bound.** A whole-pool request waits at least `--pass-for`, and after it every
job that started meanwhile must end before it starts. A long job may be passed by short ones for
up to `--pass-for`. The levels are unchanged: no request of a lower level starts while a request
of a higher level waits for slots, also when it would fit in tokens that request cannot use yet.
A whole-pool request of a higher level therefore keeps freed slots from lower levels until it
starts; only jobs of its own level use them meanwhile.

**What is shown and logged.** `Job.collected`: the slots a queued job has collected, read from
the pool in `Server::state` and once a second in `rerank` (which publishes a changed job).
`Job.ahead`: the job started or stood before a job that level, then arrival serves first; once
set it stays, and the claim takes it into the record, so the job log shows which jobs were moved
ahead. `Job.predicted_ms`: the prediction at enqueue, absent without history, so the share of
jobs predicted short that ran over 30 s can be read from `jobs.jsonl`. `slot status` ends a
queued line with `, moved ahead` and `, holds 11 of 17`, and the pool line with `, 11 collected
by queued jobs`; the web UI's queue shows `11/17` in its slots column and `ahead` beside the
level. `Job.queue_class`: the request's class in the pool (0 unpassable, 1 short, 2 the others),
read like `collected`; `slot status` (`render`) and the web UI (`servedQueue`) list the queue by
level, then this class, then arrival, which is the order the pool serves. A job without it (from
an older daemon) counts as 1 when `ahead` and 2 otherwise, so an older daemon's queue keeps the
order of level, then arrival. All four fields are `#[serde(default)]` and left out of the JSON
when empty; the handover carries them in the job and recomputes them.

**Lost tokens.** Collected tokens stay booked: they are a waiting request's `got`, which
`Pool::placed` counts (§ 21.1), and the lost-token watch writes back nothing for them.

**Not chosen.**

- *A separate bound per rule.* One bound reads the same for both: no job waits more than
  `--pass-for` because a later one passed it.
- *A run-time setting.* `--pass-for` is an option of `slot serve`; a setting like `slot priority
  promote-after` can follow when a host needs to change it without a restart.
- *Backfill by predicted end (EASY backfilling).* It would let a small job pass a large one only
  when it ends before the running jobs do. The prediction is missing for 44.5 % of the jobs, and a
  bound in time needs none.

## 26. Admission by share (0.25.0)

**The problem.** A slot is a promise of one CPU that most jobs do not keep. A test job that
sleeps on a socket holds its slot at 0 CPU, and a `cargo build` of a workspace that asked for 4
slots runs 16 compilers in a compile cache server. Over 47,572 runs of the job log of one 20-CPU
host (10.6 days to 2026-11-22), 11.3 % used under 0.1 CPU over their run; of the 8,069 that ran a
minute or longer, 10.2 %. At the peak, 13 of 39 running jobs were such jobs, and up to 17 ran at
once. Slots count what a job asked for; the CPUs count what it does.

**The setting** (`shares.rs`, `server/limits.rs`). Admission is `slots` (the default) or
`shares`, saved beside the headroom in `limits.json` as `admission` (absent by slots: the file
is then byte for byte what a daemon without the setting writes), with `share_floor`,
`share_max`, `share_pressure_max` and `share_min_budget`. `GET` and `PUT /api/admission`, `slot
admission [shares|slots] [--floor N] [--max N] [--pressure P] [--min-budget CPUS]`,
`State.shares` and the tick's `shares` (both absent by slots), one header line in `slot status`.
A floor or a most under 1, a floor over the most, a pressure mark outside (0, 100] and a lower
bound of the budget under 1 CPU are refused. By slots nothing of this section runs: the pool,
its queue and every job's record are what they were, except for the jobs that started by share
before the switch (below, *Switching back*).

The daemon applies the saved setting in `Server::new` and again in `resume`, after the running
jobs of a handover are adopted and before any queued job is queued again
(`Server::apply_saved_admission`): with `admission: shares` saved, the pool's FIFO is limited to
the budget and the share test is in place before the first request, so no job starts by slots
before the sampler's first look.

**A job's share** (`shares::share_for`, `admission::History::predicted_share`). The CPUs a job is
expected to use: `cpu_s ÷ run time` of the earlier runs of the same repository and argv that
ended on their own and ran a second or longer (the last 20), weighted by age with a half-life of
12 h, the weighted 90th percentile, plus 15 %; at least 0.1, at most the budget's cap. `cpu_s`
counts the compilers a compile cache server ran for the job (`compile_server_work`, § 24). With
fewer than 3 runs the share is 1.0. A request for slots (`--tokens N`, clients 0.21.0 and older
included) is read as a share of N CPUs, at most the budget at enqueue and the tokens in
circulation (`Server::share_ask_cap`); a request over that is told so ("slot: --tokens 16 capped
at a share of 15.0 CPUs, the budget"), so a share never waits for a budget it cannot fit. The
share is fixed at enqueue and shown as `Job.share`; when the budget later falls under a queued
job's share, that job starts only under the floor.

**Admission** (`jobserver::Pool::distribute_shares`, `shares::room`). The pool keeps its queue
and its order (level, then unpassable, short and other requests, then arrival, § 25) and changes
only the test for "fits": a request fits while

1. fewer than `share_max` jobs run (48 by default), and
2. fewer than `share_floor` jobs run (4 by default), or its share is at most the free share and,
   while the brake holds, it is predicted short.

The walk stops and passes as in § 25. The free share is
`budget − busy − starting`:

- *busy*: the CPUs of the pool and the compile cache (`Parts::counted`, § 24) over the last 5
  samples of the census sampler (`RecentParts::mean_last`). While that figure is unknown (the
  first look after a start or a reload) or stale (the sampler more than 5 s late,
  `cpu_limit::STALE_MS`), *busy* is 0 and every running job counts with its whole share, as one
  that just started: the budget is never free only because nothing was measured.
- *starting*: for each job that started under 5 s ago or was let through and has not claimed
  yet, its share less what its samples over those 5 s show, at least 0. A job counts in *busy*
  once its CPU is in the samples, and in *starting* until then, so the figure has no gap.
- *idle*: a running job whose last 2 samples are each under 0.1 CPU counts
  with what it uses, not with its share: it is in neither *starting* nor anything else, and the
  queue gets its share. `Job.lent_s` adds up, each look, its share less what it used, times the
  seconds since the last look. When it uses CPU again its CPU is in *busy* within a sample, and
  no further job starts until the free share covers one. The loans outstanding at once are bound
  by `share_max`: 48, over the 39 jobs that ran at once at the peak of the log above (the pool
  had 59 slots then). A lender that bursts is seen in *busy* at one fifth per sample: *busy* is
  the mean of 5 samples, so a job that goes from 0 to 8 CPUs adds 1.6 to *busy* at the first
  look and its whole 8 only after 5 s. Jobs that start in those seconds can take the budget over
  by up to the burst less its fifth. Not changed: the brake and the next looks hold the queue
  until *busy* falls again.

The daemon looks once a sampler tick (1 s) and after a change of the setting
(`Server::share_step`): it computes the free share and hands it to the pool, which lets through
what fits and keeps the rest for the requests that come before the next look. A queued job starts
within one tick of the free share reaching its share. Each queued job's `wait_reason` says what
it waits for ("waits for a share of 4.0 CPUs: 3.0 of a budget of 15.0 free", "48 jobs run, the
most by share", or the brake).

**The budget** (`shares::Budget`). It starts at 0.75 of the host's CPUs (15 of 20) and is never
above its cap: the pool's CPU quota where one is set, else the host's CPUs less the headroom
(§ 13). Every 5 s it looks at the slowdowns of the jobs that ended since: wall time over the
best kept wall time of the same repository and argv (`History::best_runtime`, only when that is
a second or longer). A median under 1.3 raises it by 0.5, over 2 multiplies it by 0.85.
Switching to shares starts it anew.

The best wall time counts only runs that ended ok and were not served from a cache: of the last
20 ok runs of the command, those whose `cpu_s` is at least half the median `cpu_s` of those runs
(`admission::CACHE_HIT_FRACTION`). A failed run that stopped early, or a build whose units all
came from the compile cache, would otherwise set a best that every real run misses by 2 or more.

The budget has a lower bound: `share_min_budget` (`--min-budget`, `min_budget` in the API),
by default half the host's CPUs (10 of 20), and never under the summed shares of the oldest
`share_floor` running jobs, so the floor's jobs always fit; the cap wins over both.
`GET /api/admission` and `State.shares` show it as `min_budget`; `slot status` adds "(its lower
bound)" after the budget while the budget sits there.

*Replay* of the job log (49,145 records of one 20-CPU host to 2026-11-23, cap 18, start 15; a
temporary awk script over `jobs.jsonl`, not part of the repository). The log was recorded by
slots, so the slowdowns come from the load of the host, not from the budget; the replay shows
where the rule would have put the budget, not what the budget would have done to the jobs.

| rule | slowdowns over 2 | budget lowest | 5 s windows at 2 CPUs or less |
|---|---|---|---|
| best from ok and failed runs, bound 1 | 5,515 of 15,018 (37 %) | 1.0 | 3,307 of 12,059 (27 %) |
| best from ok runs, no cache hits, bound 1 | 4,720 of 15,062 (31 %) | 1.0 | 2,530 of 11,963 (21 %) |
| the same, bound 10 (the default) | 4,720 of 15,062 (31 %) | 10.0 | 0; at the bound 4,444 (37 %) |

The cache-hit rule removes a sixth of the slowdowns over 2 and a fifth of the windows at 2 CPUs
or less; the lower bound removes the rest of the fall. With the bound the budget still sits at
it in 37 % of the windows: on this log the rule lowers the budget about as often as it raises
it (4,272 against 5,440 windows), because the host's own load moves the slowdowns. The
controller is to be measured with shares on, on one host, before admission by share is switched
on anywhere else.

**The FIFO.** A job let through by share takes no token of the FIFO and
holds only the implicit one every jobserver client has. The tokens in circulation are the pool
less the headroom, and by share at most the budget rounded (`Pool::set_limit`). The compilers
and test threads inside jobs are therefore bound by one implicit token per running job plus the
budget's tokens in the FIFO, not by the budget alone: with 12 jobs running and a budget of 15,
up to 27 processes can run at once. The free share, measured from the CPUs, is what keeps
further jobs out while they do. Switching hands every token a
queued request gathered back to the FIFO or the held-back set; `Pool::placed` and
`slot_tokens_unaccounted` count them as before. `Pool::lend` (urgent requests borrow held-back
tokens) does not run by share: an urgent job starts first in the queue and is bound by the floor
and the free share like any other.

*Switching back.* A job that started by share holds no token, so back by slots the pool would
run its whole FIFO beside it. Until it ends, one token for each such job is held back besides
the headroom (`Pool::set_owed`, from the look of each tick): missing ones are taken as tokens
return, and each is given back when its job ends. An urgent request may still borrow them
(`Pool::lend`). A handover gives the owed tokens back to the FIFO; the new image counts the jobs
again at its first look.

**The brake** (`share_pressure_max`, 60 % by default). Over this CPU pressure of the host
(`/proc/pressure/cpu`, `some avg10`) only jobs predicted short (§ 25, at most 5 s) start; the
floor still runs. The default from `census.jsonl` of the same host (34,190 one-minute windows to
2026-11-22): the pressure's median was 12.1 %, its 75th percentile 41.7 %, its 90th 70.9 %. With
15 or more of the 20 CPUs busy the median was 62.2 % (9,005 windows), with 18 or more 76.8 %. 60
lies where the host is nearly full; it held in 15.3 % of the windows (over 50: 20.3 %, over 40:
26.1 %).

**Memory.** By share the admission by memory (§ 14, `--min-free-memory`, a job's predicted peak)
holds a job as it does by slots: it runs before the pool's queue, so a job short of memory waits
there with its reason and never reaches the share test.

**What is shown.** `Job.share` (asked), `Job.share_used` (its CPUs over the last 5 samples, while
it runs; at the end its `cpu_s ÷ run time`) and `Job.lent_s`, all `#[serde(default)]` and left
out of the JSON when empty. `slot status` prints `shares: budget 15.0 of 17.0 CPUs, 3.2 free (9.0
busy, 2.8 for jobs that just started), 12 running (4 to 48), 2 idle lend 2.0` and in each job
line `share 2.3, uses 0.4`. The web view names the column `share` and shows `used/asked`, and
the header's second row the free share, the budget and its cap. `Job.admission` records the
admission a job started under (`shares`, absent by slots, `#[serde(default)]`). `slot usage`
charges a job that started by share its share times its run, less the CPU-seconds it lent; the
job that ran on the loan pays its own share. A job that spans a switch is charged by the
admission it started under. The handover state file is unchanged: a job's share is recomputed
from the record it carries, and its admission is in that record.

An unknown `/api/` path answers 404 for every method (`server/http.rs`, `fallback`), so a
`PUT /api/admission` to a daemon without the setting reads as an older daemon, not as a refused
method; another method on a page path answers 405.

**Not built.**

- *A CPU weight per job*: a child cgroup per job with its own
  `cpu.weight`, so a job over its share yields to one under it. It needs cgroup delegation of
  `slot.service` (`Delegate=yes` in a drop-in of the user manager), which needs root.
- *A second queue order*: shortest-predicted-first and least-attained-service
  are what § 25 already does inside a level with short jobs first and the pass bound.
- *A flag that names a share.* `--tokens N` is read as a share of N.

## 27. Jobs in a sandbox (`slot serve --sandbox`)

**The problem.** Every job runs as the daemon's user, and that user can write its whole home
directory: the toolchains in `~/.rustup` and `~/.cargo/bin`, every other workspace and target
directory, the compile cache's store. One build script or test can delete, replace or install a
program another build needs, or change another build's tree, and nothing tells the two apart
afterwards. On a build node every job is a synced workspace of some checkout, so the node is
where it matters first.

**The option** (`sandbox.rs`, `slot serve --sandbox`, `--sandbox-write <path>` repeated). The
daemon starts every job under bubblewrap: `bwrap`, its options, `--`, the job's command. A
daemon option, not a setting in `limits.json`: a switch that `PUT /api/...` could turn off would
let anything that reaches the HTTP port lift the sandbox. A reload keeps it, because the new image
gets the old one's arguments (§ 15.2).

**Refusal, never a silent fallback.** At start the daemon looks for `bwrap` on `PATH` (then
`/usr/bin`) and runs `true` in the namespaces and mounts every job gets. When `bwrap` is missing
or cannot create them (unprivileged user namespaces off), `slot serve` exits non-zero and names
the package and the sysctl; it never runs a job outside the sandbox. It prints
`slot: every job runs in a sandbox (<bwrap>)` to the journal when it starts.

**What a job gets.**

- The root bound read-only (`--ro-bind / /`), so the home directory, `~/.rustup`,
  `~/.cargo/bin`, `/usr`, `/usr/local` and every other workspace are read-only.
- A pid namespace and an IPC namespace of its own (`--unshare-pid --unshare-ipc`), a fresh
  `/proc` and a minimal `/dev` with a private `/dev/shm`. The job sees only its own processes
  and cannot signal one outside. bwrap's own init is pid 1; when it ends, every process left in
  the namespace ends with it.
- The network of the host. A build needs no other: the compile cache server answers on
  `127.0.0.1:4226`, and cargo may fetch crates. Tests should need no network (workspace
  `AGENTS.md`), which a sandbox does not enforce.

**Writable**, each bound at its own path; a path that does not exist is left out unless the
daemon creates it (column *created*):

| path | why | created |
|---|---|---|
| the job's checkout: `<ws-root>/<id>` for a synced workspace; for a local job the nearest ancestor of its directory with a `.git` entry below the home directory (a home kept in git is not a checkout), else the directory | a build writes generated files and `Cargo.lock` in its tree | no |
| the job's target directory: `<cache-root>/target/<id>`, or for a local job `--target-dir`, `CARGO_TARGET_DIR` or the workspace's `target/` | what cargo builds | yes |
| `/tmp`, a private tmpfs; `TMPDIR=/tmp` in the job's environment | temporary files of compilers and tests; gone with the job, so nothing is left to clean and no job reads another's | (tmpfs) |
| `$CARGO_HOME/registry`, `$CARGO_HOME/git` | cargo's download cache of crates and git dependencies, shared by every build | when `CARGO_HOME` exists |
| `$CARGO_HOME/.package-cache`, `.package-cache-mutate` | cargo's lock on that cache, so two jobs that download at once still take turns. The rest of `CARGO_HOME` stays read-only: `bin/`, `config.toml`, credentials; a job cannot add a `config.toml` that names a `rustc-wrapper` for the builds after it | when `CARGO_HOME` exists |
| `GOCACHE` (`<cache-root>/go-build` for a synced workspace; else `$XDG_CACHE_HOME/go-build`) | Go's build cache, content-addressed | when named in the environment |
| `GOMODCACHE` (else the first `GOPATH` entry's `pkg/mod`, else `~/go/pkg/mod`) | Go's module download cache | no |
| each `--sandbox-write` path | what a build of that host must write beyond these: a package manager's store, a browser download | no |

The compile cache's store stays read-only: rustc's wrapper sends each compile to the sccache
server over TCP, and the server, outside the sandbox, writes its own store. A job never writes
the home directory itself, of the job's environment or of the daemon, nor a directory that holds
one: a job started there gets its directory read-only.

**Hidden**, an empty tmpfs over each, then the job's own paths bound again on top: the workspace
root (other synced workspaces), `<cache-root>/target` (other jobs' target directories), and the
user's runtime directory (`XDG_RUNTIME_DIR`: the user bus, through which `systemd-run --user`
would start anything outside the sandbox, and the daemon's socket). The daemon's socket is also
bound to `/dev/null` wherever it lives, so a job cannot submit a job of its own with another
directory. Writable paths are bound before the hidden ones are covered and again after, so a
checkout that holds the workspace root does not bring the other workspaces back.

**What keeps working.**

- The jobserver: a FIFO stays writable on a read-only mount (the kernel's read-only check covers
  regular files and directories, not FIFOs and sockets), and its path is bound again after the
  hidden ones in case one of them or `/tmp` held it. The job's output pipes are descriptors and
  cross the sandbox unchanged.
- `--memory-max`: `systemd-run --scope` wraps `bwrap`, not the reverse, because the scope needs the
  user bus the sandbox hides. The caller's open-file limit, which `slot __exec` sets, is set
  before `bwrap` starts and is inherited.
- The daemon's view of a job: it starts `bwrap`, samples its process tree in `/proc` of the host,
  where the job's processes have host pids, and on a cancel signals the process group, which
  `bwrap` and the job share (no `--new-session`, no `--die-with-parent`: the latter fires when the
  daemon's spawning thread ends or the daemon re-executes on reload, and would kill jobs a
  handover passes on).
- `SLOT_SANDBOX=bwrap` in the job's environment says it is sandboxed.

**Fixed paths** (`--sandbox-fixed-paths`, off by default). A compile cache keys a unit by what
the compiler sees, and paths are part of it: the working directory, `CARGO_MANIFEST_DIR`, the
target directory. Two worktrees of one repository differ in every one of these, so a unit built
in one is built again in the other. With the option the job sees its checkout at `/work/src` and
its target directory at `/work/target`, starts at its own directory under `/work/src`, and gets
`CARGO_TARGET_DIR=/work/target`; every worktree of a repository then builds at the same paths.
The host's paths stay bound as without it, and the output of a synced workspace's job is
rewritten from `/work/src` to the client's root.

- The root is then not `--ro-bind / /` but a tmpfs that holds each entry of the host's `/` bound
  read-only (a symbolic link as a link), because a read-only `/` has no place to create `/work`
  (`bwrap: Can't mkdir parents for /work/src: Read-only file system`). The tmpfs itself is
  writable and private to the job; it is gone with it.
- The job runs without a rustc wrapper (`RUSTC_WRAPPER` and `CARGO_BUILD_RUSTC_WRAPPER` set
  empty). The compile cache server runs outside the sandbox: it reads the sources and runs
  rustc by the paths the job names, and `/work/src` does not exist there. Measured 2026-11-24 by
  hand against the host's server: a library crate at `/work/src` with `RUSTC_WRAPPER=sccache`
  failed (`sccache … rustc` exited 254); a binary crate built, because sccache does not cache a
  binary and runs rustc in the job. No test covers it: it needs a live server. A cache that
  runs inside the job's sandbox would serve these paths; none is built, and no second server is
  started. This departs from the workspace rule that every Rust build compiles through the
  host's compile cache, which is why the option is off by default. § 28 measures what lifts it:
  sccache's client-side mode, and a view of the target directory alone.

**Build nodes.** Amazon Linux 2023 has no `bwrap` by default. The node boot script
(`docs/design/remote-pool.md` § 7.3, step 3) installs it with the other packages:

```sh
dnf install -y bubblewrap
```

and step 11 adds `--sandbox` to `slot serve`. The probe at start tells whether the kernel there
admits unprivileged user namespaces; with them off the daemon does not start.

**Measured** (2026-11-24, one workstation, bubblewrap 0.13.0): a job under the sandbox could not
write `~/.cargo/bin/x`, could not move a file in another checkout, wrote its tree, target
directory, `/tmp` and `/dev/shm`, and wrote the pool FIFO; `cargo fetch` of two crates from
crates.io succeeded with `CARGO_HOME` read-only but `registry/`, `git/` and the two lock files.

**Not covered.**

- The network is the host's, and with it abstract Unix sockets: a job can reach every service
  on the host's loopback.
- A writable shared cache (cargo's registry, Go's caches) can be changed by one job for the
  next; the same as without the sandbox, but now the only such place.
- Sessions on the workstation that run commands outside slot are not affected; and on a
  workstation, `slot task check` of a repository that writes a package manager's store
  (`pnpm`, Playwright's browsers) fails until that store is named with `--sandbox-write`.
- `slot gate` (§ 19): its hold process writes `<id>.env` in the gate directory, so a sandboxed
  daemon serving a gate names each gate directory with `--sandbox-write`.
- A `build.target-dir` set in a `.cargo/config.toml` is not read: such a target directory outside
  the checkout is read-only to the job.
- The private `/tmp` is memory, and counts against the job's `--memory-max` scope; a node's
  disk for temporary files (`docs/design/remote-pool.md` § 7.3, step 2) is not used for it. A
  test suite that writes gigabytes there needs a per-job directory on disk instead.
- A sccache server started by a job, where no server runs, starts inside the sandbox: it cannot
  write its store and ends with the job. Run sccache as a service on a sandboxed host.

## 28. One target path that every job sees (measured 2026-11-26, not built)

Nothing in this section is built in slot. The view below was made by a probe inside its own jobs
(`contrib/cache-key-probe.sh both`), on one build node: Amazon Linux 2023, kernel 6.18,
bubblewrap 0.12.0, `unshare` of util-linux 2.37.4, sccache 0.18.0, rustc 1.100.0,
`user.max_user_namespaces` 252898, the daemon without `--sandbox`, same-path mirrors. A statement
marked *unverified* was neither measured nor read in a source.

**The problem, in numbers** (measured the same day on one lane of a consumer repository,
wasm32-unknown-unknown, 93 rustc calls through the cache, eight crates of one git dependency on
the critical path):

- First build in a worktree: 1 min 46 s, the eight crates 86.67 s. Again in the same worktree on
  an empty target directory: 18.15 s, the eight crates 11.03 s. In a second, new worktree:
  2 min 13 s, the eight crates 105.61 s; nothing was served.
- With every `CARGO_*` variable held equal and only the target path different, 41 of 67 library
  units were served and 26 compiled again: three crates that include a file from their build
  script's output directory, and every crate that depends on one of them. cargo sets `OUT_DIR`
  to an absolute path below the target directory; rustc records it (`# env-dep:OUT_DIR=…` in
  dep-info), and sccache hashes every such variable and the contents of every extern into the
  key (sccache 0.18.0, `src/compiler/rust.rs`, `generate_hash_key`).
- With the target directory at one absolute path for two worktrees the second one got the eight
  crates in 5.77 s instead of 105.61 s.
- The same on a small graph, in two trees with no `CARGO_TARGET_DIR` in either job:
  `serde_core` 1.0.229 (includes from `OUT_DIR`) 1.61 s then 1.57 s, `serde_json` 1.0.154 (depends
  on it) 0.69 s then 0.69 s; on wasm32-unknown-unknown 1.65 s then 1.74 s and 0.71 s then 0.74 s.
  `itoa`, which has no such dependency, was served: 0.11 s then 0.03 s. The probe's mode `tree`
  on the same graph, as two cargo jobs of slot 0.39.1 (each carries its own `CARGO_TARGET_DIR`):
  `serde_json` 0.65 s then 0.67 s, on wasm32-unknown-unknown 0.63 s then 0.64 s, exit 1.

**The condition.** Each worktree keeps its own target data. One directory shared by the
worktrees of a repository is ruled out: two branches in it overwrite each other's test binaries
and wait on cargo's lock. The probe's negative control (`PROBE_SHARE_TARGET=1`: one directory
bound for three worktrees) shows it: worktree 3 found everything fresh and ran the test binary
of worktree 2 (`its test printed its own branch's value 0 time(s), another worktree's 1 time(s)
(worktree 2)`, exit 1). The one absolute path is therefore a view: the job's own `<tree>/target`,
bound at that path in a mount namespace of that job alone.

**The mechanism, as the probe makes it.** The job's command runs under

```sh
bwrap --dev-bind / / --tmpfs /mnt --bind <tree>/target /mnt/slot-probe-target -- <command>
```

with `CARGO_TARGET_DIR=/mnt/slot-probe-target` in its environment, the same value in every
worktree. The root is the host's. A tmpfs covers the parent of the path in this job's view only,
so the mount point needs no directory on the host and no root. Without bwrap the probe uses
`unshare --user --map-root-user --mount` and two `mount` calls; the build is then root of a new
user namespace, where bwrap keeps the user. Both ran unprivileged in a job. Sources stay at
their own paths.

**The mechanism in slot** (a proposal). A daemon option, off by default, names the path. The
daemon wraps a job as it wraps one for `--sandbox` (§ 27: `systemd-run --scope` around `bwrap`,
no new session, the process group shared for a cancel) and sets `CARGO_TARGET_DIR` to the path.

- A cargo job: the request already carries the tree's target directory (§ 3.2a,
  `workspace.target_dir`); that directory is bound.
- A generic job that runs cargo inside (`slot task check`): the request's target directory is
  empty today for a command that is not cargo. The daemon would bind `target` beside the nearest
  lock file, as the client resolves it for a cargo job. A tree with two workspaces (a second
  manifest with a `target` of its own) would then build both into the one bound directory:
  *unverified*.
- A job whose caller named the directory (`CARGO_TARGET_DIR`, `--target-dir`): no view. It builds
  where it said.
- With `--sandbox` the two options join the sandbox's own; a tmpfs over a directory of the
  read-only root with a bind on top is what the sandbox does for its hidden paths. Not run for
  this path. A view made inside a sandboxed job (bwrap in bwrap) was not run either: the node's
  daemon runs without `--sandbox`.

**What the daemon reads: unchanged.** The job record's `target_dir`, eviction (§ 18) and disk
admission (§ 14.1) keep the real `<tree>/target`. A bind mount is the same filesystem, so
`statvfs` answers the same. The products of a remote cargo job are copied from the real
directory; the paths of cargo's `compiler-artifact` messages, from which the client takes the
file list (`artifacts.rs`), would name the view and need the prefix replaced (read, not run).

**What a job sees.**

- Source paths in compiler output are the tree's own: `Compiling fx v0.0.0 (<tree>)`, and
  diagnostics with them. Sources are not bound anywhere else.
- Paths below the target directory name the view: `Timing report saved to
  /mnt/slot-probe-target/cargo-timings/cargo-timing-….html`, `Running tests/where_.rs
  (/mnt/slot-probe-target/debug/…/where_-f175ac0373cd504f)`. The file is at the same place below
  `<tree>/target`: replacing the prefix `<path>/` by `<tree>/target/` maps every such path back,
  and the daemon would do that in the job's output as `--sandbox-fixed-paths` does for
  `/work/src`.
- `cargo metadata` answers `target_directory: /mnt/slot-probe-target` in the view and
  `<tree>/target` outside it.

**What breaks.**

- A test binary run outside the view. Built in the view, `env!("CARGO_TARGET_TMPDIR")` is
  `/mnt/slot-probe-target/tmp`: the test passed in the view (`exists=true`) and the same binary
  failed outside it (`exists=false`, `test result: FAILED`). The same holds for `env!("OUT_DIR")`
  read at run time and for `CARGO_BIN_EXE_<name>` (*unverified*, same mechanism). The products
  the client copies back to a workstation are such binaries.
- Dep-info files name the view: 3 absolute paths below `/mnt/slot-probe-target` in the dep-info
  of a workspace crate. A cargo that later builds in the same real directory
  without the view sees another `OUT_DIR` and builds those units again: *unverified*. One real
  directory should be built always in the view or never.
- A test that asserts the target directory lies below the repository root fails in the view (a
  consumer reports one; not run). A build script that walks up from `OUT_DIR` to find the
  workspace root finds `/mnt`: *unverified*.
- Debuggers and coverage tools: a source generated into `OUT_DIR` is recorded at the view's
  path, and so is every object path of split debug info; outside the view they do not resolve.
  *Unverified*; builds on this host carry no debug info.
- A consumer of `cargo metadata` outside the job (an editor) gets the real path and is not
  affected. One inside the job gets the view, which holds only there.
- Whatever the job would read below the parent of the path (`/mnt` here) is hidden from it.
- Not served, as before: the workspace's own crates (their working directory is the tree; the
  probe's own crate took 0.20 s then 0.19 s), a second profile name, a variable the caller sets
  such as a runner, check and clippy builds (not measured).

**The compile cache server is outside the view.** That decides the options.

- In sccache's default mode the client sends the command line, the working directory and the
  environment (`src/commands.rs`, `do_compile` and `request_compile`), and the server computes
  the key and runs rustc itself (`src/server.rs`, `SccacheService::handle_compile`;
  `src/compiler/compiler.rs`, `get_cached_or_compile`). The request arrives (during the
  build the server's `Compile requests` went from 7759 to 7766 and its `Compilation failures`
  from 71 to 73; the server counts every job of the node), and the
  server's rustc does not see the path: ``error: error writing dependencies to
  `/mnt/slot-probe-target/debug/build/itoa/0c0ea665ad493427/out/itoa-0c0ea665ad493427.d`: No such
  file or directory (os error 2)``, cargo exit 101. Which of the server's rustc calls printed it
  (the dep-info pass of `get_source_files_and_env_deps` or the compile) was not determined.
- A build script, a test binary and a proc macro are compiled in the view all the same: the
  server answers `UnhandledCompile` for what it does not cache and the client runs rustc
  (`src/commands.rs`, `handle_compile_result`). Only cacheable library units fail.
- With the path present outside the view and writable for the server's user, the build failed
  with the same line and 0 files were written outside: the directories cargo made exist only in
  the view.
- `SCCACHE_CLIENT_SIDE=1` in the job (sccache 0.18.0, `docs/Architecture.md`, "Client-side
  mode"; `src/commands.rs`, `do_compile_client_side`; `src/cache/ipc_storage.rs`, `IpcStorage`):
  the client computes the key and runs rustc in the job, and the server only reads and writes
  the store and adds up the statistics. The server is neither restarted nor configured for it.
  sccache stays in its default mode when `SCCACHE_LOG` is set for the client or a distributed
  scheduler is configured (`src/config.rs`; the architecture page names `SCCACHE_ERROR_LOG`).

**Options, measured.** One small graph: `serde_json` 1.0.154 and `toml_edit` 0.22.27 with their
dependencies, each build with a `--cfg` name of its own; times of the named unit from cargo's
timing report, the first build and then the build in another worktree.

| option | host target | wasm32-unknown-unknown | result |
|---|---|---|---|
| no view (today), no `CARGO_TARGET_DIR` in either job | `serde_core` 1.61 s, 1.57 s; `serde_json` 0.69 s, 0.69 s | 1.65 s, 1.74 s; 0.71 s, 0.74 s | compiled again |
| view, server in its default mode | exit 101 | not run | fails, line above |
| view, `SCCACHE_CLIENT_SIDE=1`, the shared server | `serde_core` 1.59 s, 0.25 s; `serde_json` 0.73 s, 0.17 s | 2.67 s, 0.27 s; 0.85 s, 0.18 s | served; the server's Rust hits rose by 5 during the build of 5 units |
| view, a private server inside the view for each build, one store of its own | `serde_core` 1.81 s, 0.18 s; `serde_json` 0.71 s, 0.10 s | not run | served; 5 hits |
| view of the target directory and of the sources, `SCCACHE_CLIENT_SIDE=1` | as the third row, and the tree's own crate 0.20 s, 0.11 s | not run | the tree's own crate served too |

- The probe's mode `both` with the third option: worktree 1 alone, then worktrees 2 and 3 at
  the same time. `serde_json` 0.73 s, then 0.17 s and 0.17 s; `toml_edit` 1.40 s, then 0.18 s and
  0.17 s; on wasm32-unknown-unknown `serde_json` 0.73 s, then 0.17 s and 0.23 s, `toml_edit`
  1.39 s, then 0.17 s and 0.17 s. Each worktree ran the test binary of its own branch (on wasm32
  it was built, not run: no runner), and no build log has a line waiting for a file lock. With
  `unshare` in place of bwrap: `toml_edit` 1.65 s, then 0.18 s and 0.19 s.
- What the client-side mode costs: about 0.1 s more for each rustc call that goes through the
  cache, hit or compile. Three rounds without a view, the server's mode against the client's:
  `itoa` compiled in 0.10 s, 0.10 s, 0.10 s against 0.20 s, 0.19 s, 0.23 s and was served in
  0.03 s, 0.03 s, 0.03 s against 0.12 s, 0.12 s, 0.17 s; a build of five library units took
  2.20 s to 2.43 s against 2.69 s to 2.97 s. Each client process finds out about the compiler by
  itself, where the server remembers it between requests (`src/server.rs`, `compiler_info`);
  that this is the 0.1 s is *unverified*. A unit that compiles in under about half a second is
  therefore not three times faster when it is served, which is why the probe counted 2 or 3 of
  6 library units as "at most a third of their first compile" while the server counted a hit
  for each.
- Not measured: the private server against the shared store (two servers on one disk store,
  each with its own account of the size: *unverified*); a build of the consumer's size; and
  seeding a new tree's target directory with the dependency units of a built sibling, which
  needs no view.

**Recommended:** the third option, a view of the target directory alone with
`SCCACHE_CLIENT_SIDE=1` set by the daemon in the jobs that get the view, as a daemon option that
is off by default. It is the only variant measured to keep both the shared cache server and one
target directory for each worktree, and it changes nothing in how the server runs. Its price is
the 0.1 s for each rustc call above, against the 105.61 s of the eight crates in each new
worktree. Binding the sources too would also serve a workspace's own crates, and it moves every
source path in the output: that is `--sandbox-fixed-paths` with the wrapper kept, a second step.

**Open before it is built.**

- The watchdog of § 12 (idea 7) counts the server's child processes as compiles and `sccache`
  processes as clients. In the client-side mode the compiles are children of the clients, so its
  condition "more clients than compiles" holds all the time; whether the other conditions still
  keep it from writing tokens is *unverified*.
- The CPU and memory of a compile then count in the job's own process tree and scope, not in
  the server's (`compile_server_cpu_s`, § 24): *unverified*.
- The path: its parent must exist on every node (`/mnt` here, owned by root, not writable for
  the build user).
