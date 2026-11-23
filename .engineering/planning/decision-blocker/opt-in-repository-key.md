---
format: aep.planning-md/3
id: decision-blocker:opt-in-repository-key
kind: decision-blocker
status: open
title: Nobody has decided what identifies a repository in the cluster opt-in
relations:
- blocks: story:placement-sticks-to-a-node
revision: 1
---
# Decision blocker: what identifies a repository in the opt-in

`story:placement-sticks-to-a-node` opts repositories in with `slot node allow <repository>`
(`docs/design/remote-pool.md` § 3.2 rule 3, § 7.5), and nothing in the design or the code says
what `<repository>` is. `spec/domains/cluster.yaml` types it `slot.cluster.RepositoryKey` (text)
with an `UNMAPPED:` marker.

## The question

Which key does the opt-in use?

- (A, recommended) the repository's origin URL as `git remote get-url origin` prints it,
  normalised (no credentials, no `.git` suffix): one opt-in covers every worktree and clone of
  the repository, and it is what the daemon's usage table already reads for project names.
- (B) the absolute path of the primary checkout: every worktree of it shares the git common
  directory, but a second clone needs its own opt-in.
- (C) a name the operator chooses, mapped to paths in the host configuration.

## Clears when

The operator picks one; the spec's `RepositoryKey` loses its `UNMAPPED:` marker and gains the
rule (for example a `prefix:` or an alphabet), and `ess specify validate --path spec` passes.
