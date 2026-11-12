---
format: aep.planning-md/3
id: story:docs-site-live
kind: story
status: draft
title: The documentation site answers at beyond10x.github.io/slot
relations:
- decomposes: epic:publish-first-release
- serves: vision:O6
- depends_on: story:public-repository
- depends_on: story:gates-enrolment
scope:
- confidence: inferred
  path: website/docusaurus.config.ts
- confidence: inferred
  path: website/product.json
revision: 5
---
# Story: docs-site-live

## Outcome

The documentation site built from `website/` (Docusaurus on `@beyond10x/docs-system`) is served at
<https://beyond10x.github.io/slot/> by `.github/workflows/pages.yml`, and its CLI reference is the
generated `website/docs/reference/cli.md` of the deployed commit.

## Context

- At adoption the site is in progress in `website/` and not deployed; `README.md` § Documentation
  and `website/product.json` already link the URL and the pages `getting-started`,
  `how-it-works`, `agent-integration` and `reference/cli`.
- This story deploys; it does not write pages. Pages are edited by the stories that change what
  they describe (`story:permanent-service-unit` and `story:builds-run-at-low-cpu-weight`:
  `getting-started.md`; `story:path-shim`: `agent-integration.md`; `story:prometheus-metrics`: a
  metrics page and `sidebars.ts`).
- Depends on `story:public-repository` and `story:gates-enrolment` (Pages needs the public
  repository and a green `main`).

## Acceptance

1. `curl -fsS -o /dev/null -w '%{http_code}' https://beyond10x.github.io/slot/` prints `200`, and
   the same for `docs/getting-started`, `docs/how-it-works`, `docs/agent-integration` and
   `docs/reference/cli`.
2. Every link in `website/product.json` resolves on the live site with HTTP 200.
3. The latest `pages.yml` run on `main` concluded `success`.
4. Every `--flag` named in `website/docs/reference/cli.md` at the deployed commit appears in the
   text of the live `docs/reference/cli` page (a script compares the two lists; none missing).

## Scope

- `website/docusaurus.config.ts`, `website/product.json` (base URL, links)
- `.github/workflows/pages.yml`, `.github/workflows/b10x-docs-site.yml` only if the first deploy
  exposes a defect
