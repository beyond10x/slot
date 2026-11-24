// Regression assets of closed findings that the layout contract and the screen-only user do not hold: one test per finding.
import { expect, test } from '@playwright/test';
import type { Locator, Page } from '@playwright/test';
import { open } from './harness.ts';
import type { Opened } from './harness.ts';
import { NODE16, QUIET_SESSION, SCENARIOS } from './scenarios.ts';
import type { Scenario } from './scenarios.ts';
import { CLUSTER_GATING, VIEWPORTS, vpName } from './viewports.ts';

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
  await expect(heading).toHaveText('Queue: waiting for disk space, memory and a target directory, by level, oldest first');
  await expect(heading).not.toContainText('waiting for slots');
});

/** Installed before page scripts: every height the header takes, from the first paint on. */
function headerHeights(): void {
  const w = window as unknown as { __uxHdr: number[] };
  w.__uxHdr = [];
  const note = (): void => {
    const h = document.querySelector('header')?.getBoundingClientRect().height ?? 0;
    if (h > 0 && w.__uxHdr[w.__uxHdr.length - 1] !== Math.round(h)) w.__uxHdr.push(Math.round(h));
    requestAnimationFrame(note);
  };
  requestAnimationFrame(note);
}

for (const size of GATING) {
  test(`UXF-023 @ ${vpName(size)}: the header keeps one height from the first paint on, in every state, at most 13 % of the screen`, async ({ page, baseURL }) => {
    await page.addInitScript(headerHeights);
    const seen: Record<string, number[]> = {};
    for (const name of ['machine-headroom', 'video-call-old-daemon', 'upgrade-pending', 'call-mode', 'crowded']) {
      const run = await start(page, baseURL, name, size);
      if (name === 'machine-headroom') await run.step();
      else await page.waitForTimeout(150);
      if (name === 'video-call-old-daemon') {
        await page.getByRole('button', { name: /^\s*40\s*%/ }).click();
        await expect(page.locator('header')).toContainText('older than this page');
      }
      seen[name] = await page.evaluate(() => (window as unknown as { __uxHdr: number[] }).__uxHdr);
      await page.unrouteAll();
    }
    const heights = [...new Set(Object.values(seen).flat())];
    expect(heights, `header heights seen: ${JSON.stringify(seen)}`).toHaveLength(1);
    expect(heights[0] / size.height, 'share of the screen').toBeLessThanOrEqual(0.13);
  });
}

test('UXF-024 call-mode: a wait reason is text of the Queue, and the header explains itself behind a control the keyboard opens', async ({ page, baseURL }) => {
  await start(page, baseURL, 'call-mode');
  const queue = tableOf(page, 'Queue');
  await expect(queue.getByText('free space on the build filesystem is below 25 GB')).toBeVisible();
  await expect(queue.getByText('available memory is below what the job needs')).toBeVisible();
  const header = page.locator('header');
  await expect(header.getByText(/back 2 min after the call/)).toBeVisible();
  const help = header.getByRole('button', { name: 'what the header shows' });
  await help.focus();
  await page.keyboard.press('Enter');
  const text = page.getByRole('region', { name: 'what the header shows' });
  await expect(text).toBeVisible();
  await expect(text).toContainText('kept free for you');
  await expect(text).toContainText('not on slot');
  await page.keyboard.press('Escape');
  await expect(text).toBeHidden();
  await expect(help).toBeFocused();
  await expect(page.getByRole('heading', { name: /^Queue/ }).locator('xpath=following-sibling::p[1]')).toContainText('finished in the last 60 min');
});

test('UXF-024 crowded: a cut command, its directory and its session are text of the output pane, uncut', async ({ page, baseURL }) => {
  await start(page, baseURL, 'crowded');
  await page.getByRole('button', { name: 'output of job 100', exact: true }).click();
  const pane = page.locator('aside');
  await expect(pane).toContainText('cargo test --workspace --all-targets --features postgres,tls-rustls,metrics,tracing-subscriber -- --test-threads 4 --nocapture');
  await expect(pane).toContainText('/work/ledger');
  await expect(pane).toContainText('0b1f9e2a-6c1d-4d2a-9f5e-1a2b3c4d5e6f');
  const cut = await pane.evaluate((el) =>
    [...el.querySelectorAll('*')].filter((e) => e.tagName !== 'PRE' && !e.closest('pre') && getComputedStyle(e).textOverflow === 'ellipsis' && e.scrollWidth > e.clientWidth + 1).length);
  expect(cut, 'cut texts in the pane outside the output').toBe(0);
});

test('UXF-025 queue-position: the pane of a queued job says queued, its place and its wait, and not "live"', async ({ page, baseURL }) => {
  await start(page, baseURL, 'queue-position');
  await page.getByRole('button', { name: 'output of job 79', exact: true }).click();
  const pane = page.locator('aside');
  await expect(pane).toContainText('queued');
  await expect(pane).toContainText('#4 of 6');
  await expect(pane).toContainText(/waiting \d+s/);
  await expect(pane).not.toContainText('live');
});

test('UXF-025 call-mode: the pane of a job that waits for disk space names the reason', async ({ page, baseURL }) => {
  await start(page, baseURL, 'call-mode');
  await page.getByRole('button', { name: 'output of job 134', exact: true }).click();
  await expect(page.locator('aside')).toContainText('free space on the build filesystem is below 25 GB');
});

test('UXF-027 queue-levels: the job tables name a session by its title, and by 8 digits only when it has none', async ({ page, baseURL }) => {
  await start(page, baseURL, 'queue-levels');
  const row = (table: string, id: number): Locator =>
    tableOf(page, table).getByRole('row').filter({ has: page.getByRole('button', { name: `output of job ${id}`, exact: true }) });
  await expect(row('Queue', 167).getByRole('cell', { name: 'docs site redesign', exact: true })).toBeVisible();
  await expect(row('Running', 160).getByRole('cell', { name: 'remote build server', exact: true })).toBeVisible();
  await expect(row('Finished', 150).getByRole('cell', { name: 'remote build server', exact: true })).toBeVisible();
  await expect(row('Queue', 168).getByRole('cell', { name: '9e8d7c6b', exact: true })).toBeVisible();
  // With the pane open the column is dropped; the pane names the session.
  await page.getByRole('button', { name: 'output of job 167', exact: true }).click();
  await expect(page.locator('aside')).toContainText('docs site redesign');
});

test('UXF-028 machine-headroom: the header says in a sentence what holds builds back, and the owners outside the pool are listed below', async ({ page, baseURL }) => {
  await start(page, baseURL, 'machine-headroom');
  const header = page.locator('header');
  await expect(header).toContainText(/machine over target: 18\.\d of 20 CPUs busy, other programs use 9\.1; builds are held to 2\.0 CPUs/);
  const text = await header.innerText();
  expect(text).not.toMatch(/PSI|\(observe\)|\(floor\)|pool quota|gave up|\.scope/);
  await expect(page.locator('main')).toContainText('session-3.scope 5.9');
  await expect(page.locator('main')).toContainText('docker 3f2a1b2c3d4e 3.2');
});

test('UXF-030 busy-failed: the timeline is the same picture half a second later, and "live" is not red', async ({ page, baseURL }) => {
  await start(page, baseURL, 'busy-failed');
  const timeline = page.getByRole('heading', { name: /^Timeline/ }).locator('xpath=following-sibling::canvas[1]');
  /** The pixel column left of the "now" line, where every running bar ends: one "r,g,b,a" per row. */
  const edge = async (): Promise<string[]> => {
    await page.waitForTimeout(120);
    return timeline.evaluate((el) => {
      const c = el as HTMLCanvasElement;
      const d = c.getContext('2d')!.getImageData(c.width - 2, 0, 1, c.height).data;
      const out: string[] = [];
      for (let i = 0; i < d.length; i += 4) out.push(`${d[i]},${d[i + 1]},${d[i + 2]},${d[i + 3]}`);
      return out;
    });
  };
  const at = await page.evaluate(() => Date.now());
  const a = await edge();
  await page.clock.setFixedTime(at + 500);
  const b = await edge();
  // Rows of a running bar are opaque in the first frame; the rest is a waiting bar, paper or a grid line, which moves with the time.
  const bar = a.map((px, y) => (px.endsWith(',255') ? y : -1)).filter((y) => y >= 0);
  expect(bar.length, 'rows of running bars at the right edge').toBeGreaterThan(20);
  expect(bar.filter((y) => a[y] !== b[y]), 'rows whose edge changed within 500 ms').toEqual([]);
  /** How much red stands out of a colour: 0 for a grey. */
  const redness = (l: Locator): Promise<number> =>
    l.evaluate((el) => {
      const [r, g, b2] = (getComputedStyle(el).color.match(/\d+/g) ?? []).map(Number);
      return r - Math.max(g, b2);
    });
  const live = tableOf(page, 'Running').getByRole('button', { name: /live/ }).first();
  expect(await redness(live), 'redness of a "live" button').toBeLessThan(40);
  await live.click();
  expect(await redness(page.locator('aside .status')), 'redness of the pane status of a running job').toBeLessThan(40);
});

test('UXF-031 busy-failed: the page is set at 14 px, the output at 13 px, and no text is below 12 px', async ({ page, baseURL }) => {
  await start(page, baseURL, 'busy-failed');
  await page.getByRole('button', { name: 'output of job 37', exact: true }).click();
  await expect(page.locator('aside pre')).toBeVisible();
  const sizes = await page.evaluate(() => {
    const px = (el: Element | null): number => (el ? parseFloat(getComputedStyle(el).fontSize) : 0);
    let min = Infinity;
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    for (let n = walker.nextNode(); n; n = walker.nextNode()) {
      const p = n.parentElement;
      if (!p || !n.textContent?.trim() || p.getBoundingClientRect().width === 0) continue;
      min = Math.min(min, px(p));
    }
    const least = (sel: string): number => Math.min(...[...document.querySelectorAll(sel)].map(px));
    return {
      body: px(document.body), min, output: px(document.querySelector('aside pre')),
      job: least('button.jobid'), numbers: least('main td.num'), header: least('header b'),
    };
  });
  expect(sizes.body, 'body').toBe(14);
  expect(sizes.output, 'output text').toBeGreaterThanOrEqual(13);
  expect(sizes.job, 'job numbers').toBeGreaterThanOrEqual(14);
  expect(sizes.numbers, 'numbers of the tables').toBeGreaterThanOrEqual(14);
  expect(sizes.header, 'numbers of the header').toBeGreaterThanOrEqual(14);
  expect(sizes.min, 'smallest text').toBeGreaterThanOrEqual(12);
});

for (const name of ['crowded', 'busy-failed']) {
  test(`UXF-032 ${name}: the page does not move while it loads`, async ({ page, baseURL }) => {
    await page.addInitScript(() => {
      const w = window as unknown as { __uxCls: number };
      w.__uxCls = 0;
      new PerformanceObserver((list) => {
        for (const e of list.getEntries() as Array<PerformanceEntry & { value: number; hadRecentInput: boolean }>) if (!e.hadRecentInput) w.__uxCls += e.value;
      }).observe({ type: 'layout-shift', buffered: true });
    });
    await start(page, baseURL, name);
    await page.waitForTimeout(600);
    const shift = await page.evaluate(() => (window as unknown as { __uxCls: number }).__uxCls);
    expect(shift, 'layout shift from the first paint to the loaded page').toBeLessThanOrEqual(0.01);
  });
}

for (const size of GATING) {
  test(`UXF-033 crowded @ ${vpName(size)}: the Queue has no column that repeats the reason, and no command of the queue is cut`, async ({ page, baseURL }) => {
    await start(page, baseURL, 'crowded', size);
    const queue = tableOf(page, 'Queue');
    await expect(queue.getByRole('columnheader', { name: 'waits for' })).toHaveCount(0);
    const got = await queue.evaluate((table) => {
      const box = table.getBoundingClientRect();
      const heads = [...table.querySelectorAll('thead th')].filter((th) => (th as HTMLElement).offsetWidth > 0);
      const last = heads[heads.length - 1];
      const cmds = [...table.querySelectorAll('td.cmd')] as HTMLElement[];
      return {
        last: last.textContent?.trim(),
        gap: Math.round(box.right - last.getBoundingClientRect().right),
        cut: cmds.filter((c) => c.scrollWidth > c.clientWidth).map((c) => c.textContent),
        width: Math.min(...cmds.map((c) => c.clientWidth)),
      };
    });
    expect(got.last, 'last column of the Queue').toBe('wait');
    expect(got.gap, 'room right of the wait column').toBeLessThanOrEqual(1);
    expect(got.cut, 'commands cut in the Queue').toEqual([]);
    expect(got.width, 'width of the command column').toBeGreaterThanOrEqual(600);
  });
}

test('UXF-033 crowded @ 1920x1080: where every command fits, slots and wait of the Queue stand under those of Running', async ({ page, baseURL }) => {
  await start(page, baseURL, 'crowded', { width: 1920, height: 1080, gating: false });
  const edges = (table: Locator): Promise<{ slots: number; wait: number; cut: number }> =>
    table.evaluate((t) => ({
      slots: Math.round(t.querySelector('thead th.c-slots')?.getBoundingClientRect().right ?? 0),
      wait: Math.round(t.querySelector('thead th.c-wait')?.getBoundingClientRect().right ?? 0),
      cut: ([...t.querySelectorAll('td.cmd')] as HTMLElement[]).filter((c) => c.scrollWidth > c.clientWidth).length,
    }));
  const queue = await edges(tableOf(page, 'Queue'));
  const running = await edges(tableOf(page, 'Running'));
  expect(queue.cut, 'commands cut in the Queue').toBe(0);
  expect({ slots: queue.slots, wait: queue.wait }).toEqual({ slots: running.slots, wait: running.wait });
});

test('UXF-033 call-mode: the reason of a wait is said once per job, as the line under its row', async ({ page, baseURL }) => {
  await start(page, baseURL, 'call-mode');
  const queue = tableOf(page, 'Queue');
  await expect(queue.getByText('waits for disk space: free space on the build filesystem is below 25 GB')).toBeVisible();
  expect(((await queue.innerText()).match(/disk space/g) ?? []).length, 'times the Queue table says "disk space"').toBe(1);
  // A cargo build that waits for another job of its target directory says which job.
  await expect(queue.getByText('waits for its target directory: in use by job 131')).toBeVisible();
});

test('UXF-034 busy-failed: every row of the Sessions table shows its id, and a session without a title says so', async ({ page, baseURL }) => {
  await start(page, baseURL, 'busy-failed');
  const sessions = tableOf(page, 'Sessions');
  const titled = sessions.getByRole('row').filter({ hasText: 'remote build server' });
  await expect(titled.getByRole('cell').nth(1)).toHaveText('0b1f9e2a');
  const untitled = sessions.getByRole('row').filter({ hasText: '9e8d7c6b' });
  await expect(untitled.getByRole('cell').nth(0)).toHaveText('no title');
  await expect(untitled.getByRole('cell').nth(1)).toHaveText('9e8d7c6b');
});

for (const size of GATING) {
  test(`UXF-037 busy-failed @ ${vpName(size)}: the machine charts come first, then Running, Queue and Finished`, async ({ page, baseURL }) => {
    await start(page, baseURL, 'busy-failed', size);
    const top = async (name: RegExp): Promise<number> => {
      const box = await page.getByRole('heading', { name }).first().boundingBox();
      if (!box) throw new Error(`no heading ${String(name)}`);
      return Math.round(box.y);
    };
    const charts = [await top(/^Cores: used/), await top(/^Pressure/), await top(/^Pool size/), await top(/^Compile cache/)];
    expect(new Set(charts).size, 'the four charts stand in one row').toBe(1);
    const heads = await page.locator('section.trend h2').evaluateAll((all) => all.map((h) => Math.round(h.getBoundingClientRect().height)));
    expect(heads.length, 'four chart headings').toBe(4);
    expect(Math.max(...heads), 'no chart heading takes a second line').toBeLessThan(30);
    const down = [charts[0], await top(/^Running/), await top(/^Queue/), await top(/^Finished/)];
    expect(new Set(down).size, 'four different heights').toBe(4);
    expect(down, 'order down the page').toEqual([...down].sort((a, b) => a - b));
    expect(down[1], 'Running starts in the upper half of the first screen').toBeLessThan(size.height / 2);
  });
}

/** `name` with one running and one finished job of node build-1d whose ids repeat a local job's, as a node daemon numbers them. */
function withNodeRows(name: string): { sc: Scenario; id: number } {
  const base = SCENARIOS.find((s) => s.name === name);
  if (!base) throw new Error(`no scenario ${name}`);
  const local = base.state.active.filter((j) => j.state === 'running').sort((a, b) => Number(b.enqueued_ms) - Number(a.enqueued_ms))[0];
  const ended = base.state.done[0];
  if (!local || !ended) throw new Error(`scenario ${name} has no running and no finished job`);
  const theirs = { node: 'build-1d', from_node: true, repo: 'node-repo', cwd: '/trees/node-repo', checkout: { kind: 'repository' as const } };
  const state = { ...base.state, active: [...base.state.active, { ...local, ...theirs }], done: [...base.state.done, { ...ended, ...theirs }] };
  return { sc: { ...base, name: `${name}-node-ids`, state }, id: Number(local.id) };
}

/** Every text, label and title in the page that names a job by a negative number. */
const negativeIds = (page: Page): Promise<string[]> =>
  page.evaluate(() => {
    const said = [document.body.innerText, ...[...document.querySelectorAll('[aria-label], [title]')].flatMap((e) => [e.getAttribute('aria-label') ?? '', e.getAttribute('title') ?? ''])];
    return said.flatMap((s) => s.match(/(?:#|job |^)-\d+/gm) ?? []);
  });

for (const name of ['busy-failed', 'cluster-remote']) {
  test(`UXF-038 ${name}: a node's job shows the positive id its node gave it, beside a local job of the same id`, async ({ page, baseURL }) => {
    await page.setViewportSize({ width: GATING[0].width, height: GATING[0].height });
    const { sc, id } = withNodeRows(name);
    await open(page, baseURL ?? '/', sc);
    await expect(page.getByRole('button', { name: `output of job ${id} on build-1d`, exact: true }).first()).toBeAttached();
    await expect(page.getByRole('button', { name: `output of job ${id}`, exact: true }).first()).toBeAttached();
    expect(await negativeIds(page), 'no job is named by a negative number').toEqual([]);
  });
}

for (const size of GATING) {
  test(`UXF-050 nodes-busy @ ${vpName(size)}: a node job's id and its node stand in the job cell, clear of the tool cell`, async ({ page, baseURL }) => {
    await start(page, baseURL, 'nodes-busy', size);
    for (const heading of ['Running', 'Queue', 'Finished']) {
      const got = await tableOf(page, heading).evaluate((table, node) => {
        const problems: string[] = [];
        let rows = 0;
        for (const b of table.querySelectorAll('tbody button.jobid')) {
          if (!(b.getAttribute('aria-label') ?? '').endsWith(` on ${node}`)) continue;
          rows++;
          const cell = b.closest('td') as HTMLElement;
          const tool = cell.nextElementSibling as HTMLElement;
          const id = b.textContent?.trim() ?? '';
          const edge = Math.min(cell.getBoundingClientRect().right, tool.getBoundingClientRect().left) + 0.5;
          // Every box in the cell, and the id's own text, end before the tool cell; a text cut with an ellipsis ends with its box.
          const range = document.createRange();
          range.selectNodeContents(b);
          const boxes = [...cell.querySelectorAll('*')].map((el) => [el.tagName.toLowerCase(), el.getBoundingClientRect()] as const);
          for (const [what, r] of [...boxes, ['the id text', range.getBoundingClientRect()] as const]) {
            if (r.width > 0 && r.right > edge) problems.push(`${id}: ${what} runs ${Math.round(r.right - edge)}px into the tool column`);
          }
          const said = cell.innerText;
          if (!said.includes(id)) problems.push(`${id}: the id is not shown`);
          const titled = [cell, ...cell.querySelectorAll('[title]')].some((e) => (e.getAttribute('title') ?? '').includes(node));
          if (!said.includes(node.slice(0, 8)) || !titled) problems.push(`${id}: the cell does not name the node`);
        }
        return { rows, problems };
      }, NODE16);
      expect(got.rows, `${heading}: rows of node jobs`).toBeGreaterThan(0);
      expect(got.problems, heading).toEqual([]);
    }
  });
}

test('UXF-051 nodes-busy: a number of this host alone says so, and Finished lists the node jobs that ended last', async ({ page, baseURL }) => {
  await start(page, baseURL, 'nodes-busy');
  await expect(page.locator('[data-ux-region="slots"]')).toContainText('this host');
  await expect(page.getByRole('heading', { name: /^Cores: used/ })).toContainText('this host');
  await expect(page.getByRole('heading', { name: /^Share of the pool/ })).toContainText('this host');
  const sum = page.getByRole('heading', { name: /^Running/ }).locator('xpath=following-sibling::p[1]');
  await expect(sum).toContainText(`4 on ${NODE16}`);
  await expect(sum).toContainText(/2 on this host use \d+\.\d cores/);
  const finished = tableOf(page, 'Finished');
  await expect(finished.getByRole('button').first()).toHaveAccessibleName(`output of job 1234505 on ${NODE16}`);
  await expect(finished.getByRole('button', { name: new RegExp(` on ${NODE16}$`) })).toHaveCount(6);
});

test('UXF-052 nodes-busy and cluster-queue-empty: a job\'s session is its title wherever the daemon knows one, and a job without one says so', async ({ page, baseURL }) => {
  await start(page, baseURL, 'nodes-busy');
  const row = (id: number): Locator =>
    tableOf(page, 'Running').getByRole('row').filter({ has: page.getByRole('button', { name: `output of job ${id} on ${NODE16}`, exact: true }) });
  await expect(row(1234567).getByRole('cell', { name: QUIET_SESSION.title, exact: true })).toBeVisible();
  await expect(row(1234568).getByRole('cell', { name: 'no session', exact: true })).toBeVisible();
  await page.unrouteAll();
  await start(page, baseURL, 'cluster-queue-empty');
  const list = page.getByRole('table', { name: 'Jobs, newest first' });
  await expect(list.getByRole('columnheader', { name: 'session', exact: true })).toHaveCount(1);
  const listed = (id: number): Locator => list.getByRole('row').filter({ has: page.getByRole('button', { name: `output of job ${id} on ${NODE16}`, exact: true }) });
  await expect(listed(1234567).getByRole('cell', { name: QUIET_SESSION.title, exact: true })).toBeVisible();
  await expect(listed(1234568).getByRole('cell', { name: 'no session', exact: true })).toBeVisible();
});

for (const size of CLUSTER_GATING) {
  test(`UXF-053 @ ${vpName(size)}: the start page stacks; nothing in its header overlaps, every node name and card is shown in full`, async ({ page, baseURL }) => {
    for (const name of ['cluster-remote', 'cluster-queue-empty']) {
      await start(page, baseURL, name, size);
      await expect(page.locator('.node')).toHaveCount(2);
      await page.waitForTimeout(100);
      const got = await page.evaluate(() => {
        const meet = (a: DOMRect, b: DOMRect): boolean =>
          a.left < b.right - 0.5 && b.left < a.right - 0.5 && a.top < b.bottom - 0.5 && b.top < a.bottom - 0.5;
        const header = document.querySelector('header.top') as HTMLElement;
        const parts = [header.querySelector('h1'), ...header.querySelectorAll('.kpi'), header.querySelector('nav')].filter((e): e is Element => !!e);
        const overlaps: string[] = [];
        parts.forEach((a, i) => parts.slice(i + 1).forEach((b) => {
          if (meet(a.getBoundingClientRect(), b.getBoundingClientRect())) overlaps.push(`${a.textContent?.trim().slice(0, 20)} / ${b.textContent?.trim().slice(0, 20)}`);
        }));
        const cut = [...header.querySelectorAll<HTMLElement>('.kpi, h1 > span')].filter((e) => e.scrollWidth > e.clientWidth + 1).map((e) => e.textContent?.trim());
        const names = [...document.querySelectorAll<HTMLElement>('.node .nname')].filter((e) => e.scrollWidth > e.clientWidth + 1).map((e) => e.textContent);
        const cards: string[] = [];
        for (const card of document.querySelectorAll<HTMLElement>('.node')) {
          const c = card.getBoundingClientRect();
          if (c.left < 0 || c.right > innerWidth + 0.5) cards.push(`${card.dataset.node} outside the window`);
          for (const el of card.querySelectorAll('*')) {
            const r = el.getBoundingClientRect();
            if (r.width > 0 && r.right > c.right + 0.5) cards.push(`${card.dataset.node}: ${el.tagName.toLowerCase()}.${el.getAttribute('class') ?? ''} past the card`);
          }
        }
        return { hscroll: document.documentElement.scrollWidth - innerWidth, overlaps, cut, names, cards: cards.slice(0, 5) };
      });
      expect(got, name).toEqual({ hscroll: 0, overlaps: [], cut: [], names: [], cards: [] });
      await page.unrouteAll();
    }
  });
}

test('UXF-054 cluster-queue-empty: with no job queued the endpoint panel says where the jobs run', async ({ page, baseURL }) => {
  await start(page, baseURL, 'cluster-queue-empty');
  const ep = page.locator('.endpoint');
  await expect(ep).toContainText('Queue empty');
  const line = (name: string): Locator => ep.getByRole('listitem').filter({ hasText: name });
  await expect(line('workstation')).toContainText('2 running');
  await expect(line(NODE16)).toContainText('4 running');
  await expect(line(NODE16)).toContainText('5/51 slots');
});
