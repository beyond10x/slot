// The screen-only reader runs every task from the landing page, by pointer and by keyboard.
import { expect, test } from '@playwright/test';
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { run } from './bot.ts';
import type { Mode } from './bot.ts';
import { openPage } from './probes.ts';
import { TASKS } from './tasks.ts';
import { viewportsInRun, vpName } from './viewports.ts';

const out = join(dirname(fileURLToPath(import.meta.url)), 'out', 'bots');
mkdirSync(out, { recursive: true });

/** Actions a reader may need for one task: three link clicks from the landing page. */
const MAX_TASK_ACTIONS = 3;

for (const task of TASKS) {
  for (const vp of viewportsInRun()) {
    for (const mode of ['pointer', 'keyboard'] as Mode[]) {
      test(`${task.id} @ ${vpName(vp)} by ${mode}`, async ({ page }) => {
        await page.setViewportSize({ width: vp.width, height: vp.height });
        await openPage(page, '', 'light');
        const res = await run(page, task, mode, vpName(vp));
        writeFileSync(join(out, `${task.id}@${vpName(vp)}-${mode}.json`), JSON.stringify({ ...res, gating: vp.gating }, null, 2));
        if (vp.gating && !process.env.UX_ALL) {
          expect(res.done, res.notes.join('; ')).toBe(true);
          expect(res.actions, `${task.id}: ${res.path.join(' | ')}`).toBeLessThanOrEqual(MAX_TASK_ACTIONS);
        }
      });
    }
  }
}
