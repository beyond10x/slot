---
format: aep.planning-md/3
id: story:builds-spread-over-two-nodes
kind: story
status: active
title: Builds spread over two nodes by repository
summary: the daemon sends each repository's jobs to one node, the client does return sync, parity and lease end per node, and each node mirrors only its repositories
relations:
- serves: vision:O6
- informed_by: story:placement-sticks-to-a-node
- informed_by: story:results-come-back-from-a-node
scope:
- confidence: cited
  path: CHANGELOG.md
- confidence: cited
  path: contrib/node/push.sh
- confidence: cited
  path: contrib/node/test.sh
- confidence: cited
  path: crates/slot/src/client.rs
- confidence: cited
  path: crates/slot/src/server/spill.rs
- confidence: cited
  path: website/docs/remote-builds.md
revision: 9
transitions:
- {from: "draft", to: "proposed", at: "2026-11-25T13:14:16Z", actor: "agent:slot", revision: 8}
- {from: "proposed", to: "active", at: "2026-11-25T13:14:16Z", actor: "agent:slot", revision: 9}
---
## Outcome

Builds spread over two or more nodes of `cluster.toml` with no wrapper that names a host. The
daemon sends each job to the node its repository is assigned to, and to the node with the most
room when that node is full or gone. A job on any node gets back the files it should return,
waits for toolchain parity on that node, and is refused by name after that node's lease end. Each
node mirrors only the repositories assigned to it, so two half-sized machines (16 CPUs and about
950 GB of instance store each) hold what one machine of twice the size held.

## Context

slot 0.38.0 already lists any number of `[remote.<name>]` entries and hands a queued job to the
node with the most free slots (`crates/slot/src/server/spill.rs`, `Remotes::take`).
`contrib/node/provision.sh` and `push.sh` each take one config file, so a second node is a second
config file.

Four things still assume one node:

1. Node choice ignores the repository. A repository's target directory and its compile-cache
   entries would be built on both nodes, its first sync runs twice, and every node must mirror
   every repository. One node on 2026-11-25 used 1,072 GiB of its 1,769 GiB instance store; a
   half-sized machine has about 884 GiB.
2. Return sync, the parity wait and the refusal after a node's lease end live in a host wrapper
   in front of `slot`, for one host named in an environment file. A job the daemon hands to a node
   gets none of them.
3. `push.sh` copies every `MIRROR_DIRS` entry to the node.
4. The host wrapper sends every build to its one host itself (`SLOT_HOST`), past the daemon.

Earlier drafts on the same ground: `story:placement-sticks-to-a-node` (placement of a worktree)
and `story:results-come-back-from-a-node` (patterns of returned files). This story takes the part
of each that two nodes of one owner need now.

## Design

1. Affinity. `Remotes::take` gets the job's repository (its git top level, already in the
   request) and picks the node by rendezvous hashing over the nodes that spill: the highest
   `hash(node name, top level)` among nodes that are Ready, before their deadline and with room.
   A node that is full or gone drops out; its repositories go to the next node in the hash order,
   and come back when it returns. `slot status` prints the node each repository maps to.
2. Per-node work in the client. The client in remote mode does the return sync after the job
   (the paths the host wrapper returns today), the parity wait against the node's toolchain
   manifest, and the refusal after the node's deadline, reading the deadline from the
   `cluster.toml` entry of the node it was handed. Exit 3 keeps its meaning: the job did not run.
3. Per-node mirrors. A node config gets `REPOSITORIES` (paths under `BUILD_HOME`); `push.sh`
   mirrors those under `MIRROR_DIRS` and nothing else. The daemon's affinity and the config
   agree through one command, `slot node repos <name>`, which prints the repositories the hash
   assigns to a node; `push.sh` reads it.
4. The host wrapper stops naming a host. It keeps the checks that are the host's and leaves node
   choice to the daemon. The wrapper is a file of the host, outside this repository; its change
   is handed to the host's owner with this story.

## Acceptance

- With two `[remote.*]` entries, 20 jobs of 4 repositories each run on the node their repository
  maps to; with one node gone, all 20 run on the other one; a test in `server/spill.rs` and an
  integration test with two fake nodes.
- A job handed to a node by the daemon returns the files the host wrapper returns today, waits for
  parity, and exits 3 with the node's name after its deadline; integration tests.
- `push.sh apply` with `REPOSITORIES` copies only those trees; `test.sh` case.
- `website/docs/remote-builds.md` documents two nodes; `CHANGELOG.md` entry.

## Not in this story

- Starting or stopping nodes from slot (`story:nodes-start-on-queue-pressure`).
- Moving a repository's cache between nodes when the set of nodes changes.
