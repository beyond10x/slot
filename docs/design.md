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
  misses are the workspace's own crates and build scripts.
- `resolver.feature-unification = "workspace"` (cargo 1.99.1, stable): cargo parses the key and
  rejects bad values, but five `cargo check -p` runs over ess compiled 128 units with it and 128
  without, and left 5 `syn` variants either way. It takes effect only on nightly.

Ideas, none built yet:

1. Keep sccache on: the rust toolchain could warn in the job output when a job sets
   `RUSTC_WRAPPER` empty while sccache is configured. The warning is built
   (`sccache::cache_warnings`, unreleased); it does not ask whether sccache is configured.
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
line in `slot status`. A mark under 1 or not finite, a floor under 1 and a floor without a mark
are refused. A mark above the host's CPU count is taken and holds nothing. Released switched off.

**The hold** (`cpu_limit::verdict`, `Server::cpu_verdict`). The third check of
`Server::short_of`, after disk and memory, of kind `cpu`. A queued job that could start is held
while both are true:

1. at least `floor` jobs run (a job counts from the claim of its slots);
2. busy + starting + ahead ≥ mark.

- **busy.** `Sample::host_cores` of the census sampler (§ 11): the CPU ticks of every process in
  `/proc` over one interval, so the compilers of a compile cache server and everything outside
  the pool count. The sampler runs once a second; the hold reads the mean of its last 10 samples
  (`cpu_limit::Recent`, the span of one census log line) and scans nothing itself. One sample
  swings with every process that starts or ends; the mean of 10 moves at most a tenth of that a
  second. The sampler's first sample has no earlier reading and is left out. A mean whose last
  sample is older than 5 s is no figure. A process that starts and ends inside one interval is in
  no sample; the share of `/proc/stat` that the daemon also reads (`sys_cpu`) counts it.
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

## 25. Passing in the queue: whole-pool requests and short jobs (unreleased)

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
job may be passed by later jobs of its level or below only until it has waited this long. From
then on it starts before every job that came after it. The wait is counted from the job's
enqueue time on the daemon's clock (`Config::clock`), as the promotion delay is (§ 20), so a job
that left the queue for memory or its target directory keeps its age. `--pass-for 0`: nobody
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
on, and a later request that fits in what is at hand starts ahead of it. What is left stays
booked with one request (the one that stopped the walk, else the first not covered), so two
requests never each hold part of what one of them needs; with nobody waiting it goes back to the
FIFO. With 4 slots, one running job on 1, a 4-slot request and three 1-slot requests behind it:
the 4-slot request gathers 3, and each 1-slot request takes one of them as it queues.

**The cost of the bound.** A whole-pool request waits at least `--pass-for`, and after it every
job that started meanwhile must end before it starts. A long job may be passed by short ones for
up to `--pass-for`. The levels still come first in the order; a request that fits in the tokens a
passable request of a higher level cannot use yet may start before it, until that request is
unpassable.

**What is shown and logged.** `Job.collected`: the slots a queued job has collected, read from
the pool in `Server::state` and once a second in `rerank` (which publishes a changed job).
`Job.ahead`: the job started or stood before a job that level, then arrival serves first; once
set it stays, and the claim takes it into the record, so the job log shows which jobs were moved
ahead. `Job.predicted_ms`: the prediction at enqueue, absent without history, so the share of
jobs predicted short that ran over 30 s can be read from `jobs.jsonl`. `slot status` ends a
queued line with `, moved ahead` and `, holds 11 of 17`, and the pool line with `, 11 collected
by queued jobs`; the web UI's queue shows `11/17` in its slots column and `ahead` beside the
level. All three fields are `#[serde(default)]` and left out of the JSON when empty; the handover
carries them in the job and recomputes them.

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
