// Docs hero screenshots from the hermetic scenarios: node ux/hero.ts <output dir>.
// No daemon, so no real machine, session or repository name can appear. Needs ui/dist.
import { spawn } from 'node:child_process';
import { join, resolve } from 'node:path';
import { chromium } from '@playwright/test';
import type { Page } from '@playwright/test';
import { open } from './harness.ts';
import { SCENARIOS } from './scenarios.ts';

const PORT = 4318;
const BASE = `http://127.0.0.1:${PORT}/`;
const outDir = resolve(process.argv[2] ?? '.');
const scenario = (name: string) => {
  const sc = SCENARIOS.find((s) => s.name === name);
  if (!sc) throw new Error(`no scenario ${name}`);
  return sc;
};

async function waitForServer(): Promise<void> {
  for (let i = 0; i < 100; i++) {
    try {
      if ((await fetch(BASE)).ok) return;
    } catch {
      /* not up yet */
    }
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error('vite preview did not start');
}

async function shot(page: Page, name: string, prepare: (p: Page) => Promise<void>, file: string): Promise<void> {
  const run = await open(page, BASE, scenario(name));
  await page.getByRole('combobox', { name: /window/ }).selectOption({ label: '2 min' });
  while (await run.step()) {
    /* final state */
  }
  await prepare(page);
  await page.waitForTimeout(500);
  await page.screenshot({ path: join(outDir, file) });
}

const server = spawn('pnpm', ['exec', 'vite', 'preview', '--host', '127.0.0.1', '--port', String(PORT), '--strictPort'], {
  stdio: 'ignore',
});
try {
  await waitForServer();
  const browser = await chromium.launch(process.env.UX_CHROME ? { executablePath: process.env.UX_CHROME } : {});
  const ctx = async (): Promise<Page> => (await browser.newContext({ viewport: { width: 1600, height: 1000 } })).newPage();
  await shot(await ctx(), 'crowded', async () => undefined, 'ui-host-census.png');
  await shot(await ctx(), 'queue-position', async (p) => {
    for (let i = 0; i < 2; i++) await p.getByRole('button', { name: /^show units of job/ }).first().click();
    await p.getByRole('heading', { name: /^Queue/ }).evaluate((h) => window.scrollTo(0, h.getBoundingClientRect().top + window.scrollY - 140));
  }, 'ui-queue-live-units.png');
  await shot(await ctx(), 'busy-failed', async (p) => {
    await p.getByRole('button', { name: 'output of job 42', exact: true }).click();
    await p.getByRole('heading', { name: /^Running/ }).evaluate((h) => window.scrollTo(0, h.getBoundingClientRect().top + window.scrollY - 140));
  }, 'ui-job-output.png');
  await browser.close();
} finally {
  server.kill();
}
