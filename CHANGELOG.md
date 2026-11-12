# Changelog

## 0.1.0 — unreleased

- `slot serve`: one GNU-make jobserver FIFO shared by every job, FIFO-ordered admission,
  lost-token reconcile, job log (`jobs.jsonl`), live web UI with timeline, slot usage, cores per
  toolchain and job output.
- `slot <command…>` and `slot run`: run a build through the daemon; local fallback when the
  daemon is unreachable.
- Remote mode (`SLOT_HOST`): rsync over ssh, `slot proxy`, server-side target directories,
  output paths rewritten to the client.
- Toolchains: rust (jobserver client), go (fixed slots, `-p` follows), other (one slot, pool
  passed down).
- CPU attribution of work done inside sccache through the `SLOT_JOB` tag.
