// The screen-only user: deterministic, no model. It uses only controls it can see, found by role and name.
import type { Locator, Page } from '@playwright/test';
import type { Opened } from './harness.ts';
import type { Task } from './scenarios.ts';

export type Mode = 'pointer' | 'keyboard';

export interface BotResult {
  done: boolean;
  actions: number;
  looks: number;
  steers: number;
  time_to_act_steps: number;
  keyboard_tabs: number;
  completion_miss: boolean;
  false_complete: boolean;
  notes: string[];
}

const MAX_TABS = 400;

/** 1: in the header, which never scrolls away; 2: elsewhere in view; 3: not in view. */
async function glance(loc: Locator): Promise<1 | 2 | 3> {
  return loc.evaluate((el) => {
    const r = el.getBoundingClientRect();
    const inView = r.width > 0 && r.top >= 0 && r.left >= 0 && r.bottom <= window.innerHeight && r.right <= window.innerWidth;
    if (!inView) return 3;
    const at = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
    if (!at || !(el === at || el.contains(at))) return 3;
    return el.closest('header') ? 1 : 2;
  });
}

async function firstPresent(cands: Locator[]): Promise<Locator | null> {
  for (const c of cands) if ((await c.count()) > 0) return c.first();
  return null;
}

/** Tabs from the top of the page until `target` holds focus; the count, or -1 if it never does. */
async function tabTo(page: Page, target: Locator): Promise<number> {
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  for (let n = 1; n <= MAX_TABS; n++) {
    await page.keyboard.press('Tab');
    const on = await target.evaluate((el) => el === document.activeElement || el.contains(document.activeElement));
    if (on) return n;
  }
  return -1;
}

export async function play(page: Page, run: Opened, task: Task, mode: Mode): Promise<BotResult> {
  const r: BotResult = {
    done: false, actions: 0, looks: 0, steers: 0, time_to_act_steps: 0, keyboard_tabs: 0, completion_miss: false,
    false_complete: false, notes: [],
  };
  while (await run.step()) {
    /* the bot acts on the final state */
  }
  const reach = async (loc: Locator): Promise<boolean> => {
    const g = await glance(loc);
    r.looks += g;
    if (mode === 'keyboard') {
      const tabs = await tabTo(page, loc);
      if (tabs < 0) {
        r.notes.push('not reachable by keyboard');
        return false;
      }
      r.keyboard_tabs += tabs;
      return true;
    }
    if (g === 3) {
      await loc.scrollIntoViewIfNeeded();
      r.actions++;
      r.time_to_act_steps++;
    }
    return true;
  };

  if (task.kind === 'open-output') {
    const id = String(task.job);
    const target = await firstPresent([
      page.getByRole('button', { name: new RegExp(`(^|\\D)${id}(\\D|$).*output|output.*(^|\\D)${id}(\\D|$)`, 'i') }),
      page.getByRole('row').filter({ has: page.getByRole('cell', { name: id, exact: true }) }),
    ]);
    if (!target) {
      r.notes.push(`no control for job ${id}`);
      return r;
    }
    if (!(await reach(target))) return r;
    if (mode === 'keyboard') await page.keyboard.press('Enter');
    else await target.click();
    r.actions++;
    await page.waitForTimeout(400);
    const pane = page.locator('aside');
    const text = (await pane.count()) ? await pane.innerText() : '';
    const shown = task.expect.every((e) => text.includes(e));
    r.done = shown;
    if (!shown) r.notes.push(`pane lacks: ${task.expect.filter((e) => !text.includes(e)).join(', ')}`);
    return r;
  }

  if (task.kind === 'set-headroom') {
    const preset = page.getByRole('button', { name: new RegExp(`^\\s*${task.percent}\\s*%`) });
    const slider = page.getByRole('slider', { name: /headroom/i });
    if ((await preset.count()) > 0) {
      if (!(await reach(preset.first()))) return r;
      if (mode === 'keyboard') await page.keyboard.press('Enter');
      else await preset.first().click();
      r.actions++;
    } else if ((await slider.count()) > 0) {
      if (!(await reach(slider))) return r;
      if (mode === 'pointer') {
        await slider.focus();
        r.actions++;
      }
      for (let guard = 0; guard < 40; guard++) {
        const v = Number(await slider.inputValue());
        if (v === task.percent) break;
        await page.keyboard.press(v < task.percent ? 'ArrowRight' : 'ArrowLeft');
        r.actions++;
      }
    } else {
      r.notes.push('no headroom control');
      return r;
    }
    await page.waitForTimeout(900);
    const header = await page.locator('header').innerText();
    const sent = run.limitPuts.some((b) => (b as { headroom_percent: number }).headroom_percent === task.percent);
    const claims = header.includes(`builds may use ${Math.round((20 * (100 - task.percent)) / 100)} of 20 cores`);
    if (task.fails) {
      r.done = header.includes(task.expect);
      r.false_complete = claims;
      if (claims) r.notes.push('screen shows the new headroom as applied after the daemon refused it');
    } else {
      r.done = sent && header.includes(task.expect);
      r.completion_miss = sent && !header.includes(task.expect);
    }
    if (!r.done) r.notes.push(`header: ${header.replace(/\s+/g, ' ').slice(0, 160)}`);
    return r;
  }

  const id = String(task.job);
  const row = page
    .getByRole('row')
    .filter({ has: page.getByRole('cell', { name: id, exact: true }).or(page.getByRole('button', { name: `output of job ${id}`, exact: true })) })
    .first();
  if ((await row.count()) === 0) {
    r.notes.push(`no row for job ${id}`);
    return r;
  }
  r.looks += await glance(row);
  if ((await glance(row)) === 3) {
    await row.scrollIntoViewIfNeeded();
    r.actions++;
    r.time_to_act_steps++;
  }
  const text = await row.innerText();
  r.done = text.includes(`#${task.position}`);
  if (!r.done) r.notes.push(`row: ${text.replace(/\s+/g, ' ')}`);
  return r;
}
