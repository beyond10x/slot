---
format: aep.planning-md/3
id: story:a-node-writes-an-image-tar
kind: story
status: active
title: A build node writes an image to a tar file with buildx
relations:
- serves: vision:O6
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-27T11:09:02Z", actor: "human:operator", revision: 3}
- {from: "proposed", to: "active", at: "2026-11-27T11:09:02Z", actor: "human:operator", revision: 4}
---
## Outcome

`docker buildx build --output type=docker,dest=<file>` (an image written to a tar file) works on
a build node as it does on a CI runner. A node config key turns on the containerd image store
of the node's dockerd; `provision.sh check` names a node where it is off, `apply` turns it on.

## Why

Seen 2026-11-27: a consumer's `task image:tar` failed on a node with `ERROR: Docker exporter is
not supported for the docker driver. Switch to a different driver, or turn on the containerd
image store, and try again.` The node's only buildx builder is `default` with the `docker`
driver, and dockerd has no containerd image store. CI builds the same command.

## Design

1. Node config key `DOCKER_CONTAINERD_STORE` (`on` or `off`; default `on` for a new config,
   documented in `node.conf.example` and the README).
2. `provision.sh apply` merges `"features": {"containerd-snapshotter": true}` into
   `/etc/docker/daemon.json` (keeping every other key), and restarts docker only when the file
   changed, after the jobs that use docker have ended (the same drain the node's unit uses), or
   reports that a restart is pending.
3. `provision.sh check` reads `docker info` (`DriverStatus` / the storage driver) as the build
   user and names a node whose store is not containerd.
4. Images in the old store are not visible after the switch; the README says so.
5. `contrib/node/test.sh` covers merge, idempotence and the check under a temporary root.

## Acceptance

- Red first: a `test.sh` case with a `daemon.json` without the feature: `check` reports it.
- `apply` writes the feature once and keeps the other keys; a second `apply` changes nothing.
- On a node after apply (owner-approved): `docker buildx build --output
  type=docker,dest=x.tar .` of a two-line Dockerfile writes the file, through `slot`.
