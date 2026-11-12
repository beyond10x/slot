# D-02 triage

## Caveats

- `report.md` measures the site after the D-01 change set, at 7 sizes and both themes; the search
  page the plugin adds makes 12 pages instead of 11.
- Its 96 "search: missing" lines at the gating sizes are a probe miss, not the site: the search
  plugin's box is an `input` with `aria-label="Search"` and no `type="search"`, which the probe's
  selector did not name. The probe now names it; the box is on every page (`shots/docs@1366x768-light.png`).
- No person, no review agents, no L4 or L5.

## Did the last change sets move their metrics?

Reader runs at 1366x768 by pointer, D-01 → D-02 (actions, looks, steers):

| change | target metric | before | after |
|---|---|---|---|
| DXF-005 landing names headroom and the plugin | headroom | 2, 7, 1 | 1, 1, 0 |
| DXF-005 | plugin | 2, 8, 1 | 1, 6, 0 |
| DXF-005 at 390x844 | headroom, plugin | 4, 16, 3 and 4, 16, 3 | 1, 2, 0 and 1, 7, 0 |
| DXF-002 underlined links | axe link-in-text-block, gating runs | 32 | 0 |
| DXF-003 24 px dots | axe target-size on the landing page, all sizes | 14 | 0 |
| DXF-004 no stale version | `ux/copy.test.ts` | fails on main | passes |
| DXF-001 search box | always-visible search, gating runs | 44 missing | present (probe fixed) |

Keyboard Tabs to a section link in the table of contents went up with the search box and the
new landing cards, 1366x768: install 69 → 89, cancel 75 → 106, remote-env 87 → 112 (final run).
Recorded as DXF-008; the ratchet holds them from here.

## Findings and priority

| finding | severity | frequency | confidence | cost | priority |
|---|---|---|---|---|---|
| DXF-007 below 997 px the search box covers the menu button (axe target-size on `.navbar__toggle`, 48 measured-only runs at 390x844 and 768x1024; the box sits at 330 to 374 px, the button at 320 to 360 px at 390x844) | S1 (9) | 12/12 | 2 (axe and the box positions) | S (1) | 18.0 |
| DXF-006 the search box's "ctrl K" hint at 1.3:1 contrast in light (axe color-contrast, 24 gating runs) | S2 (3) | 12/12 | 1 | S (1) | 3.0 |
| DXF-008 a keyboard reader needs 89 to 112 Tabs from the landing page to a table-of-contents link (the table comes after the article in tab order; 76 of them on the CLI page alone) | S3 (1) | 3/7 | 1 | L (4) | 0.11 |

DXF-007 came with the D-01 search box: the plugin pins its box to the right edge below 997 px,
where this site's menu button sits.

## Change set D-02 (3 changes)

1. DXF-007: below 997 px the search box moves left by the menu button's width and margin
   (`src/css/custom.css`); `ux/narrow.spec.ts` checks navbar, menu button and search in view and
   not covered at 390x844 on every merge.
2. DXF-006: the hint takes the text colour.
3. The probe names the plugin's search input; on narrow windows it checks the navbar, the menu
   button and the search box.

DXF-008 stays open: moving the table of contents ahead of the article changes the shared theme.

## After the change set

Final run of this branch, 7 sizes, both themes, 272 tests: 0 gate problems, 0 measured-only
problems, every reader task done by pointer and keyboard at every size. Gating run (`task
site-ux`): 82 tests passed; `website/ux/baseline.json` written from it.

The change set is `task:ux-d02-change-set` under `story:ux-loop-docs-site`.
