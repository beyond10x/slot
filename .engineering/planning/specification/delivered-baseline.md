---
format: aep.planning-md/3
id: specification:delivered-baseline
kind: specification
status: draft
title: What slot does before its first release
summary: 'Baseline at store adoption: pool daemon, client, toolchains, remote mode, web UI, job log, generators; cites files and tests.'
relations:
- serves: vision:O6
revision: 2
---
# Specification: what slot does before its first release

Recorded when the planning store was adopted (2026-11-12), so the forward work below has a
baseline to cite. Everything listed here exists in the working tree; nothing here was built under
this store. Release state: `CHANGELOG.md` § 0.1.0 is "unreleased", the repository has no commit.

## Delivered

| capability | where | covered by |
|---|---|---|
| jobserver pool daemon `slot serve`: one GNU-make jobserver FIFO, one token per slot, FIFO-ordered admission, lost/surplus-token reconcile | `crates/slot/src/jobserver.rs`, `crates/slot/src/server/mod.rs`, `crates/slot/src/cli.rs` (`ServeArgs`) | `second_job_waits_for_the_slot`, `jobserver_env_reaches_command_and_client_jobs_are_dropped`, `client_disconnect_kills_job_and_returns_token` (`crates/slot/src/server/tests.rs`) |
| client `slot <command…>` / `slot run`: streams stdout and stderr separately, passes the exit code through, runs locally with one stderr line when the daemon is unreachable | `crates/slot/src/client.rs` (`run`, `run_local`) | `streams_output_and_exit_code` |
| toolchains rust (jobserver client), go (fixed slots, `-p` follows what is held), other (one slot, pool passed down) | `crates/slot/src/toolchain.rs` (`Cargo`, `Go`, `Generic`, `registry`) | `picks_toolchain_by_command`, `cargo_env_owns_jobserver`, `go_parallelism_follows_held_tokens`, `go_job_holds_at_most_the_pool`, `client_env_is_an_allowlist` |
| CPU attribution of sccache work through `SLOT_JOB` | `crates/slot/src/server/mod.rs` (`COMPILE_SERVERS`), `crates/slot/src/procstat.rs` | `docs/design.md` § 9 (measured with the Go prototype) |
| remote mode (`SLOT_HOST`): rsync over ssh, `slot proxy`, server-side target directories, compiler paths rewritten to the client repository | `crates/slot/src/client.rs`, `crates/slot/src/rewrite.rs`, `crates/slot/src/paths.rs` | `workspace_is_confined_and_paths_rewritten`, `remote_job_gets_server_path_and_client_flags`; verified only through a local stand-in for ssh (`docs/design.md` § 9) |
| live web UI: timeline, slot usage, cores per toolchain, live job output; Vue 3 embedded at compile time | `ui/`, `crates/slot/src/server/http.rs` (`/api/state`, `/api/events`, `/api/jobs/{id}/output`), `crates/slot/build.rs` | `task ui-check` |
| job log `jobs.jsonl`: one line per job with argv, repo, session, toolchain, tokens, enqueue/start/end, exit, CPU seconds, compile-server CPU, peak RSS | `crates/slot/src/types.rs` (`Job`), `crates/slot/src/server/mod.rs` | `streams_output_and_exit_code`, `history_survives_restart` |
| generators with drift checks: CLI reference and UI TypeScript types | `crates/slot-docs/src/main.rs`, `website/docs/reference/cli.md`, `ui/src/generated/types.ts` | `task check` runs both with `--check` |
| systemd user unit and its install target: `contrib/slot.service` (`slot serve`, `Restart=on-failure`, `WantedBy=default.target`), `task service` (install, `daemon-reload`, `enable --now`) | `contrib/slot.service`, `Taskfile.yml` (`service`) | not run on the operator's workstation yet: the daemon runs there under a transient unit (`story:permanent-service-unit`) |
| gate and release workflows | `Taskfile.yml` (`check`), `.github/workflows/gate.yml`, `.github/workflows/release.yml` | not run yet: the repository has no remote |

## In progress at adoption

- Documentation site: `website/` (Docusaurus, `website/product.json`), deployed by
  `.github/workflows/pages.yml`; not live.
- agentplugins `slot` plugin: in progress in the agentplugins repository; not released.
- Prometheus `GET /metrics`: `crates/slot/src/server/metrics.rs` and its route appeared in the
  working tree while this store was drafted; tracked as `story:prometheus-metrics`.

## Not built (`docs/design.md` § 10)

`--fetch`; reflink seeding and disk-budget eviction of target directories; the ssh forced
command; per-repository `.slot.toml`; `--memory-max` exists but is off by default and untested.
Not verified (§ 9): real ssh to a second machine, `--memory-max`, 20+ concurrent jobs.

## Acceptance

This record holds while `task check` passes on the tree and every test named in the table above
exists under that name in `crates/slot/src/`; `grep -n "fn <name>" -r crates/slot/src` finds each
one.
