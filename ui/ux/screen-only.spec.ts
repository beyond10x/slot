// Gate 3 and 4: the screen-only user completes every task by pointer and by keyboard, each fix in at most 2 actions.
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, test } from '@playwright/test';
import { play } from './bot.ts';
import type { Mode } from './bot.ts';
import { open } from './harness.ts';
import { SCENARIOS } from './scenarios.ts';
import { VIEWPORTS, vpName } from './viewports.ts';

const OUT = join(dirname(fileURLToPath(import.meta.url)), 'out', 'bot');
const MODES: Mode[] = ['pointer', 'keyboard'];
const MAX_ACTIONS_PER_FIX = 2;

for (const sc of SCENARIOS) {
  const task = sc.task;
  if (!task) continue;
  for (const vp of VIEWPORTS.filter((v) => v.gating || process.env.UX_ALL)) {
    for (const mode of MODES) {
      test(`screen-only ${sc.name} @ ${vpName(vp)} by ${mode}`, async ({ page, baseURL }) => {
        await page.setViewportSize({ width: vp.width, height: vp.height });
        const run = await open(page, baseURL ?? '/', sc);
        const r = await play(page, run, task, mode);
        mkdirSync(OUT, { recursive: true });
        writeFileSync(join(OUT, `${sc.name}@${vpName(vp)}-${mode}.json`), JSON.stringify({ scenario: sc.name, viewport: vpName(vp), mode, ...r }, null, 2));
        expect(r.done, `task done (${r.notes.join('; ')})`).toBe(true);
        expect(r.completion_miss, 'completion_miss').toBe(false);
        expect(r.false_complete, 'false_complete').toBe(false);
        if (mode === 'pointer') expect(r.actions, 'actions per fix').toBeLessThanOrEqual(MAX_ACTIONS_PER_FIX);
        expect(r.notes.filter((n) => n.includes('out of sight')), 'the opening control stays in sight').toEqual([]);
      });
    }
  }
}
