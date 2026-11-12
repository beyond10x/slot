---
format: aep.planning-md/3
id: story:agentplugins-slot-plugin-released
kind: story
status: draft
title: An agentplugins release carries the slot plugin
relations:
- decomposes: epic:publish-first-release
- serves: vision:O6
- depends_on: story:first-release-0-1-0
revision: 2
---
# Story: agentplugins-slot-plugin-released

## Outcome

An agentplugins release carries a plugin named `slot`: installed in Claude Code or Codex, it gives
a session the instruction to run builds as `slot <command…>` (with `--local` as the escape) and a
way to install the `slot` CLI from release 0.1.0.

## Context

- The plugin is in progress in the agentplugins repository at adoption; it is not in that
  repository's `plugins/` on its main checkout.
- The change and the release land in agentplugins, by its own release process.
- Depends on `story:first-release-0-1-0`: the plugin installs the CLI from a published release.

## Acceptance

1. The latest agentplugins GitHub release's marketplace manifest lists a plugin named `slot`.
2. In that release, the `slot` plugin's skill or instruction file contains a build command written
   as `slot cargo` (or `slot <command…>`) and the flag `--local`; `grep -rn -e 'slot cargo' -e '--local'`
   over the plugin's directory finds both.
3. In a fresh session with that plugin installed and no `slot` binary, following the plugin's
   install step leaves `slot --version` printing `0.1.0`.

## Scope

- No change in this repository.
