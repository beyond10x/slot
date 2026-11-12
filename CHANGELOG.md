# Changelog

## Unreleased

- `contrib/slot-shim`: a `PATH` shim for `cargo` and `go` that sends build, check, test, clippy,
  doc, bench and install through slot and runs every other subcommand directly, so agent shells
  build through the pool without a `slot` prefix. Install steps in the agent-integration guide.

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
