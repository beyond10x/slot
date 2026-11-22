// Regression assets of closed findings that the layout contract and the screen-only user do not hold: one test per finding.
import { expect, test } from '@playwright/test';
import type { Locator, Page } from '@playwright/test';
import { open } from './harness.ts';
import type { Opened } from './harness.ts';
import { SCENARIOS } from './scenarios.ts';
import { VIEWPORTS, vpName } from './viewports.ts';

const GATING = VIEWPORTS.filter((v) => v.gating);

async function start(page: Page, baseURL: string | undefined, name: string, size = GATING[0]): Promise<Opened> {
  const sc = SCENARIOS.find((s) => s.name === name);
  if (!sc) throw new Error(`no scenario ${name}`);
  await page.setViewportSize({ width: size.width, height: size.height });
  return open(page, baseURL ?? '/', sc);
}

/** The table under the heading that starts with `heading`. */
const tableOf = (page: Page, heading: string): Locator =>
  page.getByRole('heading', { name: new RegExp(`^${heading}`) }).locator('xpath=following-sibling::table[1]');

test('UXF-018 busy-failed @ 960x1080: a job at the bottom of the window stays in sight when its output opens as a sheet', async ({ page, baseURL }) => {
  await start(page, baseURL, 'busy-failed', VIEWPORTS.find((v) => v.width === 960));
  const opener = page.getByRole('button', { name: 'output of job 28', exact: true });
  const pane = page.locator('aside');
  const atBottom = (): Promise<void> => opener.evaluate((el) => el.scrollIntoView({ block: 'end' }));
  const inSight = (): Promise<boolean> =>
    opener.evaluate((el) => {
      const b = el.getBoundingClientRect();
      const hit = document.elementFromPoint(b.left + b.width / 2, b.top + b.height / 2);
      return !!hit && (hit === el || el.contains(hit));
    });
  await atBottom();
  await opener.click();
  await expect(pane).toBeVisible();
  expect(await pane.evaluate((el) => getComputedStyle(el).position), 'the pane is a sheet').toBe('fixed');
  expect(await inSight(), 'opened by pointer').toBe(true);
  await page.keyboard.press('Escape');
  await expect(pane).toHaveCount(0);
  await atBottom();
  await opener.evaluate((el: HTMLElement) => el.focus({ preventScroll: true }));
  await page.keyboard.press('Enter');
  await expect(pane).toBeVisible();
  expect(await inSight(), 'opened by keyboard').toBe(true);
});

test('UXF-019 busy-failed: the header counts the failed jobs of the last 60 min and opens the newest one', async ({ page, baseURL }) => {
  const run = await start(page, baseURL, 'busy-failed');
  const failed = page.locator('header').getByRole('button', { name: /^failed \d+ \(60 min\)/ });
  await expect(failed).toHaveAccessibleName('failed 1 (60 min): output of job 37, the newest failed job');
  await run.step();
  await run.step();
  await expect(failed).toHaveText('failed 2 (60 min)');
  await expect(failed).toHaveAccessibleName('failed 2 (60 min): output of job 42, the newest failed job');
  await failed.focus();
  await page.keyboard.press('Enter');
  await expect(page.locator('aside')).toContainText('error[E0308]');
  expect(await page.evaluate(() => window.scrollY), 'opened without a scroll').toBe(0);
});

test('UXF-019 idle: no job failed, so the header shows no failed count', async ({ page, baseURL }) => {
  await start(page, baseURL, 'idle');
  await expect(page.locator('header')).toContainText('running 0');
  await expect(page.locator('header').getByRole('button', { name: /failed/ })).toHaveCount(0);
  await expect(page.locator('header')).not.toContainText('failed');
});

for (const size of GATING) {
  test(`UXF-020 @ ${vpName(size)}: the Queue heading starts inside the first screen`, async ({ page, baseURL }) => {
    for (const name of ['busy-failed', 'queue-position', 'queue-levels']) {
      await start(page, baseURL, name, size);
      const top = await page.getByRole('heading', { name: /^Queue/ }).evaluate((h) => h.getBoundingClientRect().bottom + window.scrollY);
      expect(top, `${name}: page y of the bottom of the Queue heading`).toBeLessThan(size.height);
      await page.unrouteAll();
    }
  });
}

test('UXF-021 crowded: over three ticks without input no unit row opens and no Running row moves', async ({ page, baseURL }) => {
  const run = await start(page, baseURL, 'crowded');
  const running = tableOf(page, 'Running');
  /** Offset of every job's row from the top of the Running table, by job. */
  const frame = (): Promise<Record<string, number>> =>
    running.evaluate((table) => {
      const top = table.getBoundingClientRect().top;
      const out: Record<string, number> = {};
      for (const b of table.querySelectorAll('button.jobid')) out[b.textContent ?? ''] = Math.round(b.closest('tr')!.getBoundingClientRect().top - top);
      return out;
    });
  const frames = [await frame()];
  const open = [await running.getByRole('button', { name: /^hide units of job/ }).count()];
  while (await run.step()) {
    frames.push(await frame());
    open.push(await running.getByRole('button', { name: /^hide units of job/ }).count());
  }
  expect(frames.length, 'the snapshot tick and two more').toBe(3);
  expect(open, 'unit rows open in each frame').toEqual([0, 0, 0]);
  const inAll = Object.keys(frames[0]).filter((id) => frames.every((f) => id in f));
  expect(inAll.length).toBe(14);
  for (const id of inAll) expect(frames.map((f) => f[id]), `row of job ${id}`).toEqual([frames[0][id], frames[0][id], frames[0][id]]);
  // The user's toggle still opens a row, and a tick does not close it.
  await running.getByRole('button', { name: 'show units of job 101', exact: true }).click();
  await expect(running.getByRole('button', { name: 'hide units of job 101', exact: true })).toHaveCount(1);
});

test('UXF-022 busy-failed: Enter on a job moves the focus into its output pane, Escape closes it and the focus is on the job again', async ({ page, baseURL }) => {
  const run = await start(page, baseURL, 'busy-failed');
  await run.step();
  await run.step();
  const opener = page.getByRole('button', { name: 'output of job 42', exact: true });
  const pane = page.locator('aside');
  const close = pane.getByRole('button', { name: 'close', exact: true });
  await opener.focus();
  await page.keyboard.press('Enter');
  await expect(pane).toContainText('error[E0308]');
  expect(await pane.evaluate((el) => el.contains(document.activeElement)), 'the focus is inside the pane').toBe(true);
  for (let tabs = 0; tabs < 2 && !(await close.evaluate((el) => el === document.activeElement)); tabs++) await page.keyboard.press('Tab');
  await expect(close, '"close" within 2 Tab presses').toBeFocused();
  await page.keyboard.press('Escape');
  await expect(pane).toHaveCount(0);
  await expect(opener).toBeFocused();
  // By pointer the focus stays on the job, as before; Escape closes the pane too.
  await opener.click();
  await expect(pane).toContainText('error[E0308]');
  await expect(opener).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(pane).toHaveCount(0);
});

test('UXF-029 busy-failed: a result is a word, and a build outside the pool is "not on slot" everywhere', async ({ page, baseURL }) => {
  await start(page, baseURL, 'busy-failed');
  const finished = tableOf(page, 'Finished');
  await expect(finished.getByRole('columnheader', { name: 'result', exact: true })).toHaveCount(1);
  await expect(finished.getByRole('columnheader', { name: 'exit', exact: true })).toHaveCount(0);
  const row = (id: number): Locator => finished.getByRole('row').filter({ has: page.getByRole('button', { name: `output of job ${id}`, exact: true }) });
  await expect(row(37).getByRole('cell', { name: 'failed (101)', exact: true })).toHaveCount(1);
  await expect(row(39).getByRole('cell', { name: 'ok', exact: true })).toHaveCount(1);
  await expect(page.locator('header')).toContainText('not on slot 1.1 cores');
  await expect(page.getByRole('heading', { name: /^Sessions/ })).toContainText('not on slot');
  const text = await page.locator('body').innerText();
  expect(text).not.toMatch(/unmanaged|bypass/i);
  await expect(page.getByRole('heading', { name: /^Queue/ })).toHaveText('Queue: waiting for slots, by level, oldest first');
});

test('UXF-029 call-mode: the Queue heading names disk space and memory, not slots, when the listed jobs wait for those', async ({ page, baseURL }) => {
  await start(page, baseURL, 'call-mode');
  const heading = page.getByRole('heading', { name: /^Queue/ });
  await expect(heading).toHaveText('Queue: waiting for disk space and memory, by level, oldest first');
  await expect(heading).not.toContainText('waiting for slots');
});
