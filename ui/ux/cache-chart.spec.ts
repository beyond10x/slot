// The compile cache chart of the chart row: its values, and its empty states, which keep the row's height.
import { expect, test } from '@playwright/test';
import type { Locator, Page } from '@playwright/test';
import { MINUTE_MS, cachePoint, minuteAndDay, percentOrDash } from '../src/lib/cache.ts';
import { SETTLE_FRAMES, open } from './harness.ts';
import { clsObserver, layoutShift } from './probes.ts';
import { SCENARIOS } from './scenarios.ts';
import type { Scenario } from './scenarios.ts';
import { VIEWPORTS, vpName } from './viewports.ts';

const GATING = VIEWPORTS.filter((v) => v.gating);

function scenario(name: string): Scenario {
  const sc = SCENARIOS.find((s) => s.name === name);
  if (!sc) throw new Error(`no scenario ${name}`);
  return sc;
}

const chart = (page: Page): Locator =>
  page.locator('section.trend > div').filter({ has: page.getByRole('heading', { name: /^Compile cache/ }) });

/** Pixels of the chart in the colour of each line: the blue of "of all calls", the ochre of "of cacheable calls". Grid and labels are grey. */
const ink = (page: Page): Promise<{ all: number; cacheable: number }> =>
  chart(page).locator('canvas').evaluate((el) => {
    const c = el as HTMLCanvasElement;
    const px = c.getContext('2d')!.getImageData(0, 0, c.width, c.height).data;
    let all = 0;
    let cacheable = 0;
    for (let i = 0; i < px.length; i += 4) {
      if (px[i + 3] === 0) continue;
      if (px[i + 2] - px[i] > 60) all++;
      else if (px[i] - px[i + 2] > 60) cacheable++;
    }
    return { all, cacheable };
  });

/** Where the row ends and the tables begin: the chart's height and the top of the Running heading. */
async function heights(page: Page): Promise<{ chart: number; running: number }> {
  const box = await chart(page).boundingBox();
  const running = await page.getByRole('heading', { name: /^Running/ }).boundingBox();
  if (!box || !running) throw new Error('no chart row');
  return { chart: Math.round(box.height), running: Math.round(running.y) };
}

// One page per state: UXF-037 and the layout gates hold the row at both gating sizes, so each state here is opened at one.
for (const size of GATING.slice(0, 1)) {
  test(`busy-failed @ ${vpName(size)}: the legend has the newest minute's and the 24 hours' shares and both lines are drawn`, async ({ page, baseURL }) => {
    const sc = scenario('busy-failed');
    await page.setViewportSize({ width: size.width, height: size.height });
    await open(page, baseURL ?? '/', sc);
    const minutes = sc.cacheHistory ?? [];
    const newest = minutes[minutes.length - 1];
    const now = cachePoint(newest);
    if (!sc.cacheDay) throw new Error('busy-failed answers api/cache-day');
    const day = cachePoint(sc.cacheDay);
    const legend = chart(page).locator('.legend > span');
    await expect(legend.nth(0)).toHaveText(`of all calls${minuteAndDay(now.all, day.all)}`);
    await expect(legend.nth(1)).toHaveText(`of cacheable calls${minuteAndDay(now.cacheable, day.cacheable)}`);
    expect(minuteAndDay(now.all, day.all), 'the scenario has calls in its newest minute and its day').toMatch(/^\d+ % · 24 h: \d+ %$/);
    expect(percentOrDash(now.all), 'the minute and the day differ').not.toBe(percentOrDash(day.all));
    // Each share stays on one line beside its label: a legend of more lines would move the tables below.
    for (const k of [0, 1]) {
      const box = await legend.nth(k).boundingBox();
      expect(box?.height, `legend entry ${k} takes one line`).toBeLessThan(22);
    }
    await expect(chart(page).locator('.none')).toHaveCount(0);
    await expect.poll(async () => (await ink(page)).all, 'pixels of the line "of all calls"').toBeGreaterThan(50);
    expect((await ink(page)).cacheable, 'pixels of the line "of cacheable calls"').toBeGreaterThan(50);
    const calls = newest.hits + newest.misses + newest.not_cached;
    await expect(chart(page).locator('.lines')).toHaveAttribute('title', new RegExp(`^last minute: ${calls} compiler calls, .*not cacheable \\(crate-type \\d+, incremental \\d+\\)$`));
  });
}

for (const [name, why, size] of [['fresh', 'an empty answer', GATING[0]], ['video-call-old-daemon', 'a daemon without the path (404)', GATING[1]]] as const) {
  {
    test(`${name} @ ${vpName(size)}: ${why} reads "no cache data yet", and nothing moves when the minute's request brings calls`, async ({ page, baseURL }) => {
      await page.setViewportSize({ width: size.width, height: size.height });
      await page.addInitScript(clsObserver);
      const requested = page.waitForResponse((r) => r.url().endsWith('/api/cache-history'));
      await open(page, baseURL ?? '/', scenario(name));
      expect((await requested).status(), 'answer of api/cache-history').toBe(name === 'fresh' ? 200 : 404);
      const none = chart(page).locator('.none');
      await expect(none).toHaveText('no cache data yet');
      await expect(none).toBeVisible();
      expect(await none.evaluate((el) => getComputedStyle(el).color), 'the quiet colour').toBe('rgb(92, 92, 92)');
      const legend = chart(page).locator('.legend > span');
      await expect(legend.nth(0)).toHaveText('of all calls– · 24 h: –');
      await expect(legend.nth(1)).toHaveText('of cacheable calls– · 24 h: –');
      await page.evaluate((n) => (window as unknown as { __ux: { settle(n: number): Promise<number> } }).__ux.settle(n), SETTLE_FRAMES);
      expect(await ink(page), 'no line is drawn').toEqual({ all: 0, cacheable: 0 });
      const without = await heights(page);

      // The page asks again every minute; this time the daemon has calls, and their sum over 24 hours.
      const busy = scenario('busy-failed');
      await page.route('**/api/cache-history', (r) => r.fulfill({ json: busy.cacheHistory ?? [] }));
      await page.route('**/api/cache-day', (r) => r.fulfill({ json: busy.cacheDay }));
      await page.clock.fastForward(MINUTE_MS);
      await expect(none).toHaveCount(0);
      await expect(legend.nth(0)).toHaveText(/^of all calls\d+ % · 24 h: \d+ %$/);
      await expect.poll(async () => (await ink(page)).all, 'pixels of the line "of all calls"').toBeGreaterThan(50);
      expect(await heights(page), 'the chart and the tables stand where they did without calls').toEqual(without);
      expect(await layoutShift(page), 'layout shift from the first paint to the chart with calls').toBeLessThanOrEqual(0.01);
    });
  }
}

test('idle: no call in the window shown reads so, with dashes; the 6 h window has the calls of half an hour ago', async ({ page, baseURL }) => {
  await page.setViewportSize({ width: GATING[0].width, height: GATING[0].height });
  await open(page, baseURL ?? '/', scenario('idle'));
  const none = chart(page).locator('.none');
  await expect(none).toHaveText('no compiler calls in this window');
  const legend = chart(page).locator('.legend > span');
  await expect(legend.nth(0), 'no minute now, the calls of the day').toHaveText('of all calls– · 24 h: 31 %');
  await expect(legend.nth(1)).toHaveText('of cacheable calls– · 24 h: 55 %');
  const before = await heights(page);
  await page.getByLabel('window').selectOption('21600');
  await expect(none).toHaveCount(0);
  await expect.poll(async () => (await ink(page)).all).toBeGreaterThan(0);
  await expect(legend.nth(0), 'a minute of half an hour ago is not now').toHaveText('of all calls– · 24 h: 31 %');
  expect(await heights(page)).toEqual(before);
});
