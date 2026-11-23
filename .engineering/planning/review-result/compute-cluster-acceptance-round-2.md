---
format: aep.planning-md/3
id: review-result:compute-cluster-acceptance-round-2
kind: review-result
status: active
title: Acceptance critic, compute cluster, round 2
relations:
- reviews: epic:compute-cluster
- reviews: story:cluster-design-and-spec-in-the-gate
- reviews: story:one-hand-started-node-measured
- reviews: story:host-config-file
- reviews: story:sync-holds-back-secrets
- reviews: story:daemon-knows-nodes
- reviews: story:node-health-poll
- reviews: story:cluster-view-in-web-ui
- reviews: story:placement-sticks-to-a-node
- reviews: story:node-leaves-without-stranding-jobs
- reviews: story:daemon-conforms-to-cluster-spec
- reviews: story:provider-contract-and-node-up-down
- reviews: story:node-boot-script
- reviews: story:spot-nodes-and-reclaim
- reviews: story:results-come-back-from-a-node
- reviews: story:sync-sends-the-git-file-list
- reviews: story:nodes-start-on-queue-pressure
- reviews: story:warm-caches-across-node-lives
- reviews: story:new-worktree-seeded-from-a-sibling
- reviews: story:cluster-night-measured
revision: 1
---
needs-revision

story:spot-nodes-and-reclaim — the outcome promises spot "with the price capped at on-demand" and an on-demand launch only when "no spot capacity within a minute", but acceptance 1 to 5 check neither the cap in the spot request nor the one-minute wait, so a spot request with no cap, or a fallback after any delay, still closes the story; add a check that the double's recorded spot request carries the on-demand cap and that the fallback follows the NoCapacity answer within a minute — .engineering/planning/story/spot-nodes-and-reclaim.md:46
story:warm-caches-across-node-lives — the outcome promises a node that "boots from an image with toolchains and a warm registry" and the Specification adds a provider `image` reference, but acceptance 2 names only the cache location in the spec, and acceptance 4 checks only "time to Ready ... under one minute, recorded", so the `image` field can be left out of the spec and an image without toolchains or a warm registry still passes; name that the spec declares `image` and what the boot from the image is shown to hold — .engineering/planning/story/warm-caches-across-node-lives.md:56

What I read: 19 of 19 ids (`epic:compute-cluster` and its 18 draft stories, including the new `story:cluster-night-measured`). I ran `aep plan artifact show epic:compute-cluster` and `aep plan artifact show review-result:compute-cluster-acceptance-round-1` for the round-1 findings. I read the full body of each story file under `.engineering/planning/story/` with `cat -n`/`sed`. I also read `docs/design/remote-pool.md` § 9.5 and the spot lines, and `spec/domains/provisioning.yaml`.

Round 1 re-check, all six now hold:
- `story:cluster-design-and-spec-in-the-gate` acceptance 1 and 2 name the committed pattern file with its patterns and the exact `git grep` command, and `story:provider-contract-and-node-up-down` acceptance 8 cites the task.
- `story:results-come-back-from-a-node` acceptance 2 tests `--fetch` on a successful build and a failed one.
- `story:nodes-start-on-queue-pressure` acceptance 1 and 4 cover the `[auto]` spec table and the hours bound.
- `story:node-boot-script` acceptance 2 and 3 name `+240` and 30 minutes.
- `story:new-worktree-seeded-from-a-sibling` acceptance 3 and 4 are unconditional.
- `story:sync-sends-the-git-file-list` now takes `include` from the spec step of `story:results-come-back-from-a-node` acceptance 1, named in its Specification (line 29).

`story:cluster-night-measured` is acceptable: acceptance 1 to 6 are observable, they match the epic's acceptance and § 9.5 targets, and a miss files a story.

What I could not establish:
- The stand-in ssh node, the test double and the placement-frame symbols are new code, so I could not check that they exist.
- Out of my lane, and they set no part of the verdict: the two `UNMAPPED:` open questions in the epic (repository key, rerun count) block `story:placement-sticks-to-a-node` and `story:node-leaves-without-stranding-jobs`. That is scope and design.
- `story:cluster-night-measured` closes on a report that records a miss, while the epic's acceptance needs the measures met. This is a scope question for another critic.
- `story:cluster-design-and-spec-in-the-gate` acceptance 5 depends on `ess` pinning in `spec/ess-inputs.yaml`, which I did not verify.

```findings
- file: .engineering/planning/story/spot-nodes-and-reclaim.md
  line: 46
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the outcome promises spot with the price capped at on-demand and an on-demand launch only when no spot capacity is found within a minute, but acceptance 1 to 5 check neither the cap in the spot request nor the one-minute bound, so both can be missing and the story still closes"
- file: .engineering/planning/story/warm-caches-across-node-lives.md
  line: 56
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the outcome promises a boot from an image with toolchains and a warm registry and the Specification adds an image reference, but acceptance 2 names only the cache location in the spec and acceptance 4 checks only the time to Ready, so the image field and the image's contents have no check"
```
