---
format: aep.planning-md/3
id: story:the-local-gate-runs-shell-tests-under-the-forges-shell
kind: story
status: draft
title: The local gate runs its shell tests under the forge's shell
relations:
- serves: vision:O2
revision: 1
---
## Outcome

The shell tests of the local gate run under the shell the forge runs them under, so a test that
passes only under bash fails locally before it is pushed.

## Why

On 2026-11-26 the forge's `gate` went red after a green local gate, on one test of
`contrib/node/test.sh` (`refresh temporary names`). The test's stand-in for `toolchains.sh` ran
`readlink /proc/$$/fd/1 >file` to read where its output goes. dash applies a redirection in the
shell itself while the command runs, so the link named that file; bash redirects in the child.
Read with a probe: under bash the link names the output file, under dash 0.5.12 the file of the
redirection.

- The forge's runner has `/bin/sh` = dash. The workstation and a build node have `/bin/sh` =
  bash, and neither has dash.
- `task check` runs `sh contrib/slot-shim.test.sh`, `sh contrib/rustc-gate.test.sh` and `sh
  contrib/node/test.sh`: 893 tests in the last of them, every one under bash locally.
- The node's README lists `dash` among the packages Amazon Linux 2023 lacks.

## Acceptance

- Red first: with the stand-in in its old form, the local gate fails on the same test as the
  forge did.
- A node has dash, installed by `contrib/node/provision.sh` from a pinned source tarball with a
  SHA-256, in the release the forge's runner image has; `toolchains.sh` prints one line for it.
- `task check` runs the three shell tests under dash where dash is on PATH, and says in one line
  which shell ran them. Each test still runs once, not once per shell: the step's budget is
  unchanged.
- The forge's run prints the same line, with the same shell.

## Notes

- Not covered: bash-only syntax in the scripts a node runs as root (`provision.sh` runs under
  the node's `/bin/sh`, which is bash); the forge's run under dash covers what the tests reach.
