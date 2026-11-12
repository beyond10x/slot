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

### 3.3 Transport and access

- ssh with a dedicated key whose `authorized_keys` entry has
  `command="slot-shell",restrict`. `slot-shell` accepts only `sync` (rsync server side) and `run`.
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

Not built yet:
- `--fetch` of artifacts back to the client.
- Reflink seeding of a new workspace's target dir, and disk-budget eviction of target dirs.
- ssh forced-command (`command="slot-shell",restrict`); remote mode uses plain ssh to
  `slot proxy` and rsync.
- `.slot.toml` per-repo config (tests that must run `--local`).
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
   short checks are not stuck behind release builds.
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
  misses are the workspace's own crates and build scripts.
- `resolver.feature-unification = "workspace"` (cargo 1.99.1, stable): cargo parses the key and
  rejects bad values, but five `cargo check -p` runs over ess compiled 128 units with it and 128
  without, and left 5 `syn` variants either way. It takes effect only on nightly.

Ideas, none built yet:

1. Keep sccache on: the rust toolchain could warn in the job output when a job sets
   `RUSTC_WRAPPER` empty while sccache is configured.
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
  without being a call.

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


## 14. Admission by disk and memory (0.5.0)

Need: the workspace rule says no build starts under 25 GB free, and a build that runs out of disk
or memory fails late and takes others with it. Slots count CPU only.

Before a job takes slots, `Server::short_of` checks, in this order:

- **Disk.** `statvfs` of the job's directory and of its `CARGO_TARGET_DIR` (relative to the
  directory when relative), at the nearest existing ancestor. Under `--min-free-disk` (GB,
  default 25, 0 off) the job waits. Disk does not come back by waiting for other jobs, so a job
  alone waits too, until space is freed.
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
