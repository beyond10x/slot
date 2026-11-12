# Changelog

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
