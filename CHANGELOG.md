# Changelog

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
