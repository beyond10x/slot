// Layout contract (gates 1, 2 and 5): every scenario, every viewport, every state change.
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, test } from '@playwright/test';
import { open } from './harness.ts';
import { clsObserver, layoutShift, probe } from './probes.ts';
import type { Probe } from './probes.ts';
import { OPEN_PANE, SCENARIOS } from './scenarios.ts';
import { VIEWPORTS, vpName } from './viewports.ts';

const OUT = join(dirname(fileURLToPath(import.meta.url)), 'out', 'gates');

const problems = (p: Probe): string[] => [
  ...(p.doc_hscroll > 0 ? [`page scrolls sideways by ${p.doc_hscroll}px (${p.past_viewport.join('; ')})`] : []),
  ...p.past_box.map((s) => `runs past its panel: ${s}`),
  ...p.scrollers.map((s) => `scrolls: ${s}`),
  ...p.clipped_without_full_value.map((s) => `clipped without its full value: ${s}`),
  ...p.hidden_always_visible.map((s) => `always-visible ${s}`),
  ...p.axe_serious.map((s) => `axe: ${s}`),
];

for (const sc of SCENARIOS) {
  for (const vp of VIEWPORTS.filter((v) => v.gating || process.env.UX_ALL)) {
    test(`layout ${sc.name} @ ${vpName(vp)}`, async ({ page, baseURL }, info) => {
      await page.setViewportSize({ width: vp.width, height: vp.height });
      await page.addInitScript(clsObserver);
      const run = await open(page, baseURL ?? '/', sc);
      const pane = OPEN_PANE[sc.name];
      if (pane !== undefined) {
        await page.getByRole('button', { name: `output of job ${pane}`, exact: true }).first().click();
        await page.waitForTimeout(300);
      }
      const states: Array<{ moment: string; probe: Probe }> = [];
      states.push({ moment: 'opened', probe: await probe(page) });
      mkdirSync(join(OUT, 'shots'), { recursive: true });
      await page.screenshot({ path: join(OUT, 'shots', `${sc.name}@${vpName(vp)}-opened.png`) });
      while (await run.step()) {
        const moment = sc.steps[run.index()].moment;
        states.push({ moment, probe: await probe(page) });
        await page.screenshot({ path: join(OUT, 'shots', `${sc.name}@${vpName(vp)}-${moment.replace(/\W+/g, '-')}.png`) });
      }
      const cls = await layoutShift(page);
      const minutes = Math.max(run.elapsed(), 1) / 60;
      const result = {
        scenario: sc.name, viewport: vpName(vp), gating: vp.gating, states, minutes,
        layout_shift: { total: cls, per_minute: cls / minutes },
        problems: states.flatMap((s) => problems(s.probe).map((p) => `${s.moment}: ${p}`)),
      };
      mkdirSync(OUT, { recursive: true });
      writeFileSync(join(OUT, `${sc.name}@${vpName(vp)}.json`), JSON.stringify(result, null, 2));
      if (vp.gating) expect(result.problems, info.title).toEqual([]);
    });
  }
}
