# D-01 triage

## Caveats

- First docs round. The report (`report.md`) measures the site on `main` before any change:
  `website/` at merge 0db3d10, built and served from a copy, every page and every reader task at
  7 sizes, both themes.
- The probe and the reader bot were corrected before this measurement, so D-01 is the first
  comparable baseline: a grid row's negative margin and code inside a scrolling block no longer
  count as "past its box"; the sidebar is checked at the end of the article, not below the
  footer; a heading's own `#` link is not a link the reader picks; on a narrow window the reader
  opens the menu.
- No person, no review agents, no L4 or L5; the screen-only reader stands in for the cards.
- Frequency is counted over the 11 pages or the 7 reader tasks the finding touches.

## Findings and priority

| finding | severity | frequency | confidence | cost | priority |
|---|---|---|---|---|---|
| DXF-002 links in running text told apart by colour only (axe link-in-text-block), 8 of 11 pages | S2 (3) | 8/11 | 1 | S (1) | 2.18 |
| DXF-005 landing page names neither headroom nor the plugin: 2 actions, 1 steer, 7 to 8 looks at 1366x768; 4 actions, 3 steers, 16 looks at 390x844 | S2 (3) | 2/7 | 2 (reader and copy review) | S (1) | 1.71 |
| DXF-001 no search box (always-visible "search: missing" on all 44 gating runs) | S2 (3) | 11/11 | 1 | M (2) | 1.50 |
| DXF-004 landing page says 0.1.0 (eyebrow, status title); status list misses 7 features shipped in 0.3.0 to 0.5.1 | S2 (3) | 1/11 | 2 (copy test and copy review) | S (1) | 0.55 |
| DXF-003 carousel dots are 11 px targets (axe target-size), landing page | S3 (1) | 1/11 | 1 | S (1) | 0.09 |

No sideways page scroll and nothing past its box at any of the 7 sizes.

## Change set D-01 (5 changes)

1. DXF-002: `src/css/custom.css` underlines links in paragraphs, list items, table cells and quotes.
2. DXF-005: the landing page gets a **Headroom** action and feature cards for headroom with call
   mode, disk and memory admission and the Claude Code plugin; `docs/index.md` lists them under
   "What it is" and "Next".
3. DXF-001: local search (`@easyops-cn/docusaurus-search-local` 0.55.3, the version another
   beyond10x site uses), see `decision.md`.
4. DXF-004: no version on the landing page; `data/status.json` gains cancel, disk and memory
   admission, upgrade of a busy daemon, headroom, call mode, the sccache watchdog and the plugin.
   `ux/copy.test.ts` fails on main's `product.json` ("names 0.1.0, the workspace is 0.10.0").
5. DXF-003: each dot is a 24 px button around the same 11 px dot.

The change set is `task:ux-d01-change-set` under `story:ux-loop-docs-site`.
