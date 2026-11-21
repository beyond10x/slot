import { expect, test } from '@playwright/test';
import { open } from './harness.ts';
import { recordedCharts, T0 } from './scenarios.ts';

test('reload restores recorded CPU and queue, recent detail and incoming ticks', async ({ page, baseURL }) => {
  const sc = recordedCharts();
  const run = await open(page, baseURL ?? '/', sc);
  await page.reload();
  const chart = page.getByRole('img', { name: /^Host CPU and queue/ });
  await expect(chart).toHaveAccessibleName(/1 s samples: 1.*10 s recorded averages: 1$/);

  // Read pixels through an actual CPU and queue average, an uncovered gap, and recent exact data.
  const recordedInk = (secondsAgo: number) => chart.evaluate((el, age) => {
    const c = el as HTMLCanvasElement;
    const pixels = c.getContext('2d')!.getImageData(Math.round(c.width * (1 - age / 600)), 0, 1, c.height).data;
    const rgb = new Set<string>();
    for (let i = 0; i < pixels.length; i += 4) {
      if (pixels[i + 3] > 0) rgb.add([pixels[i], pixels[i + 1], pixels[i + 2]].join(','));
    }
    return [rgb.has('67,103,137'), rgb.has('120,99,154')];
  }, secondsAgo);
  await expect.poll(() => recordedInk(295)).toEqual([true, true]);
  expect(await recordedInk(250)).toEqual([false, false]);
  expect(await recordedInk(5)).toEqual([false, false]);
  await page.getByLabel('window').selectOption('21600');
  await expect(chart).toHaveAccessibleName(/10 s recorded averages: 2$/);
  await run.step();
  await expect(chart).toHaveAccessibleName(/1 s samples: 2.*10 s recorded averages: 2$/);
  await page.getByLabel('window').selectOption('120');
  await expect(chart).toHaveAccessibleName('Host CPU and queue, 1 s samples: 2');
});

test('an older daemon serves exact chart history and live queue updates', async ({ page, baseURL }) => {
  const sc = recordedCharts();
  delete sc.chartHistory;
  const run = await open(page, baseURL ?? '/', sc);
  const chart = page.getByRole('img', { name: /^Host CPU and queue/ });
  await expect(chart).toHaveAccessibleName('Host CPU and queue, 1 s samples: 1');
  await run.step();
  await expect(chart).toHaveAccessibleName('Host CPU and queue, 1 s samples: 2');
});

test('reconnect preserves earlier open-tab samples when server history has rolled over', async ({ page, baseURL }) => {
  const sc = recordedCharts();
  const run = await open(page, baseURL ?? '/', sc);
  await run.step();
  const chart = page.getByRole('img', { name: /^Host CPU and queue/ });
  await expect(chart).toHaveAccessibleName(/1 s samples: 2/);

  sc.history = [];
  sc.state.now_ms = T0 + 180_000;
  sc.chartHistory!.push({ t: T0, n: 10, host_cores: 16, waiting: 8 });
  await page.clock.setFixedTime(sc.state.now_ms);
  const response = page.waitForResponse((r) => r.url().endsWith('/api/chart-history'));
  await page.evaluate(() => (window as unknown as { __ux: { reconnect(): void } }).__ux.reconnect());
  await response;
  await expect(chart).toHaveAccessibleName('Host CPU and queue, 1 s samples: 2, 10 s recorded averages: 1');
});
