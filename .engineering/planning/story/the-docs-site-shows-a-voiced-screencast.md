---
format: aep.planning-md/3
id: story:the-docs-site-shows-a-voiced-screencast
kind: story
status: active
title: The docs site shows a one-minute voiced screencast of the cluster view, recorded from demo data
relations:
- serves: vision:O6
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-11-24T13:23:51Z", actor: "agent:slot", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-24T13:23:51Z", actor: "agent:slot", revision: 3}
---
# The docs site shows a one-minute voiced screencast of the cluster view, recorded from demo data

## Why

A reader of the docs site sees what slot's live view shows only from still screenshots. A
one-minute screencast with a voice-over of the cluster view (nodes, jobs, the pool) exists as a
prototype, but it was recorded against a live machine: it shows private checkout and session
names, home paths and a person's login, and it uses a voice whose licence was not read. It
cannot be published.

## Acceptance

- One command (`task screencast`) records the video against the built `ui/dist` with every API
  answered from demo data (the fixtures in `ui/ux/scenarios.ts` or a scenario added there):
  neutral repository, session and node names, no home path, no login. No live daemon is read.
- The recording tool is in the repository (`website/screencast/` or `ui/screencast/`): the scene
  script, the scene template, the recorder (Playwright, TypeScript or Node), the voice step and
  the encoder step (POSIX sh, ffmpeg). No Python.
- The voice is a Piper voice whose licence was read and permits publication; the page names the
  voice and its licence. The voice model is downloaded by the task into a cache outside git.
- The task writes `checks/`: frames of every scene and a report. A check fails the task when a
  frame's text (read from the DOM at that frame) holds a home path, a user name of the host, a
  host name, or a name not in the demo data.
- The published file is at most 2.5 MB (1280x720, H.264 + AAC, faststart), in
  `website/static/`, embedded on the landing page or the cluster page with a poster image and
  captions (WebVTT from the scene texts).
- The video is recorded again only when the UI it shows changed; the release checklist in
  `AGENTS.md` says when to run the task.
- The task runs outside the slot pool (it records, it does not build) except its `ui` build step.
- Red first: the privacy check fails on a frame from the existing private recording.
