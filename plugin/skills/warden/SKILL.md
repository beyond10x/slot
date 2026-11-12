---
name: warden
description: Find the agent sessions that build outside the shared slot pool and report them, or nudge each to build through slot, as /slot:warden [report|nudge|watch] or when the operator asks an agent to check which sessions bypass slot. Hands off to slot:building and its slot-warden role.
argument-hint: "[report|nudge|watch] [duration]"
---

# Warden

Run the slot-warden role of `slot:building` (§ The warden) in the mode given in the arguments:
`report` (default), `nudge` or `watch`, with an optional duration for `watch`.

Nudge and watch send messages to other sessions: when an agent, not the operator, started this,
run `report` and stop with the report; nudging needs the operator's word.
