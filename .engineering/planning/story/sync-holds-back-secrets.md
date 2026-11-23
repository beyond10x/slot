---
format: aep.planning-md/3
id: story:sync-holds-back-secrets
kind: story
status: draft
title: The sync to a node holds back secrets, and a fetch takes only regular files
relations:
- decomposes: epic:compute-cluster
- serves: vision:O2
- informed_by: executable-system-specification:compute-cluster
scope:
- confidence: inferred
  path: crates/slot/src/client.rs
- confidence: inferred
  path: crates/slot/src/toolchain.rs
revision: 3
---
# Story: the sync holds back secrets, and a fetch takes only regular files

## Outcome

What a session's tree sends to a remote node and what comes back are bounded: a deny list holds
back files that look like credentials, `.git` is excluded at every depth, environment values that
are workstation paths or URLs with a password stay behind, a fetch writes only regular files and
directories, and the client refuses an rsync older than 3.4.0.

## Specification

`slot.cluster` component `slot-daemon` (the sync is the client's leg of `SubmitJob` on a remote
node); no new declaration.

## Design

`docs/design/remote-pool.md` § 8.4 (deny list, values by shape), § 4.4 (nested `.git`, path
values), § 8.6 (fetch of regular files only, rsync version), § 9.2 (phase 1 security).

## Acceptance

1. A tree holding `.env`, `.envrc`, `a.pem`, `a.key`, `id_ed25519`, `.netrc`, `.npmrc`,
   `x.tfvars`, `x.tfstate`, `aws-credentials.txt` and `sub/.git/config` syncs to a stand-in
   mirror without any of them, and stderr names each file held back, one line each.
2. A `CARGO_HTTP_PROXY` value of the form `scheme://user:pass@host` and a `CARGO_TARGET_DIR`
   under the caller's home are not in the remote job's environment; a test reads the
   environment the stand-in node received.
3. A fetched path that is a symlink, a device or a FIFO on the node is refused with one line and
   exit non-zero; a regular file arrives.
4. With an rsync older than 3.4.0 on `PATH` the remote path is refused with one line naming the
   version, and the job runs locally.
5. Tests use the existing local stand-in for ssh; no network; each under 60 s.

## Scope

`crates/slot/src/client.rs`, `crates/slot/src/toolchain.rs`.

Shared with stories of other epics: `toolchain.rs` with `story:test-execution-bounded-by-pool`,
`client.rs` with `story:path-shim`. Neither is ordered against this one; whichever merges second
rebases.

## Not in this story

The git file list instead of `.gitignore` filters (`story:sync-sends-the-git-file-list`).
