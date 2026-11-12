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
