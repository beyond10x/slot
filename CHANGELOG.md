# Changelog

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
