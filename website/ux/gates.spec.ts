// Layout contract of the docs site, every page at every size in the run, both themes:
// no sideways page scroll, nothing past its box, navbar/search/theme toggle/sidebar in view at the
// top and the bottom of the page, axe-core with no serious or critical violation.
import { expect, test } from '@playwright/test';
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { isDocPage, sitemapPages, slug } from './pages.ts';
import { axeSerious, layout, layoutShift, openPage } from './probes.ts';
import { RATCHET_VIEWPORT, THEMES, viewportsInRun, vpName } from './viewports.ts';

const out = join(dirname(fileURLToPath(import.meta.url)), 'out', 'gates');
mkdirSync(join(out, 'shots'), { recursive: true });

export interface GateRecord {
  page: string;
  viewport: string;
  theme: string;
  gating: boolean;
  doc_hscroll: number;
  past_box: string[];
  scroll_boxes: string[];
  hidden_always_visible: string[];
  axe_serious: string[];
  layout_shift: number;
  problems: string[];
}

for (const route of sitemapPages()) {
  for (const vp of viewportsInRun()) {
    for (const theme of THEMES) {
      test(`${slug(route)} @ ${vpName(vp)} ${theme}`, async ({ page }) => {
        await page.setViewportSize({ width: vp.width, height: vp.height });
        await openPage(page, route, theme);
        const top = await layout(page, isDocPage(route));
        const cls = await layoutShift(page);
        const name = `${slug(route)}@${vpName(vp)}-${theme}`;
        if (vpName(vp) === RATCHET_VIEWPORT || !vp.gating) await page.screenshot({ path: join(out, 'shots', `${name}.png`) });
        const axe = await axeSerious(page);
        // The end of the content, not of the page: the footer below it is not what a reader reads.
        await page.evaluate(() => {
          const end = document.querySelector('article') ?? document.querySelector('main');
          const bottom = (end?.getBoundingClientRect().bottom ?? 0) + window.scrollY;
          window.scrollTo(0, Math.max(0, bottom - window.innerHeight));
        });
        await page.waitForTimeout(150);
        const bottom = await layout(page, isDocPage(route));
        const hidden = [...new Set([...top.hidden_always_visible.map((h) => `top: ${h}`), ...bottom.hidden_always_visible.map((h) => `bottom: ${h}`)])];
        const problems = [
          ...(top.doc_hscroll ? [`page scrolls sideways by ${top.doc_hscroll}px`] : []),
          ...top.past_box.map((p) => `past its box: ${p}`),
          ...hidden.map((h) => `always-visible: ${h}`),
          ...axe.map((a) => `axe: ${a}`),
        ];
        const rec: GateRecord = {
          page: route, viewport: vpName(vp), theme, gating: vp.gating,
          doc_hscroll: top.doc_hscroll, past_box: top.past_box, scroll_boxes: top.scroll_boxes,
          hidden_always_visible: hidden, axe_serious: axe, layout_shift: Math.round(cls * 1000) / 1000, problems,
        };
        writeFileSync(join(out, `${name}.json`), JSON.stringify(rec, null, 2));
        if (vp.gating && !process.env.UX_ALL) expect(problems, problems.join('\n')).toEqual([]);
      });
    }
  }
}
