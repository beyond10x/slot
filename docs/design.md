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

### 13.2 Headroom of the machine (unreleased)

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
counter every 10 ms, through two reloads, the second right after the first handover finished:
every client got all 300 lines once and its exit code, 40 jobs logged once each, and 20 of 20
tokens in the FIFO at the end. A second test sends 6 reloads 30 ms apart to 4 running and 4
queued jobs: the daemon keeps its pid and every client gets its output and exit code. The first
version waited 1 s between the reloads and failed 1 of 5 runs on a loaded machine with one
handover logged instead of two.

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
- **When.** Every 60 s, per filesystem (device id): when free space is under `--evict-below`
  (GB, default 40, 0 off), directories go least recently built first until free space plus their
  sizes reaches the floor plus 10 GB. Sizes are measured only until that goal is met.
- **Kept.** A directory whose repository has a queued or running job, a directory a queued or
  running job builds into, and one built in the last 2 h. Before each deletion the check repeats
  under the server lock.
- **What counts as a target directory.** Absolute, at least three levels deep, a real directory
  (not a symlink), with a `CACHEDIR.TAG` whose first line is Cargo's signature. Anything else is
  never deleted, whatever the log says.
- **How.** When the directory's parent is a managed worktree (`worktree discard-cache --dry-run
  --json <tree>` exits 0), `worktree discard-cache <tree>` deletes it; otherwise
  `remove_dir_all`. A job that wants a directory while it is being deleted waits with
  `slot: waiting: <dir> is being deleted to free disk space`.
- **Dry run.** `--evict-dry-run` defaults to true in the first release: the journal says
  `slot: disk: … free, under …: would delete <dir> (<size>, last built … ago)` when the list
  changes, and `slot status` shows a `disk:` line. Set it to false once the journal has shown
  only directories that may go.

