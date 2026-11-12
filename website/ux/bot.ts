// The screen-only reader: deterministic code, no model. It starts on the landing page and uses only
// what is on screen: links fully inside the viewport and not covered at their centre, found by their
// accessible name. A click (or Enter on a focused link) is one action, a scroll by one window is one
// look, a link taken only because nothing names the task is one steer.
import type { Page } from '@playwright/test';
import { BASE_PATH } from './pages.ts';
import type { Task } from './tasks.ts';
import { FALLBACK_LINKS } from './tasks.ts';

export type Mode = 'pointer' | 'keyboard';

export interface BotResult {
  task: string;
  viewport: string;
  mode: Mode;
  done: boolean;
  actions: number;
  looks: number;
  steers: number;
  keyboard_tabs: number;
  path: string[];
  notes: string[];
}

const MAX_ACTIONS = 8;
const MAX_LOOKS_PER_PAGE = 12;

interface SeenLink {
  index: number;
  name: string;
  route: string;
  hash: string;
}

/** Visible same-site links in document order, with their route relative to the site base. */
async function visibleLinks(page: Page): Promise<SeenLink[]> {
  return page.evaluate((base) => {
    const vw = document.documentElement.clientWidth;
    const vh = window.innerHeight;
    const out: Array<{ index: number; name: string; route: string; hash: string }> = [];
    const all = [...document.querySelectorAll('a[href]')];
    all.forEach((a, index) => {
      // A heading's own '#' link is not navigation a reader picks.
      if (a.classList.contains('hash-link')) return;
      const url = new URL((a as HTMLAnchorElement).href, location.href);
      if (url.origin !== location.origin || !url.pathname.startsWith(base)) return;
      const r = a.getBoundingClientRect();
      if (r.width === 0 || r.height === 0 || r.left < 0 || r.top < 0 || r.right > vw + 0.5 || r.bottom > vh + 0.5) return;
      const at = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
      if (!at || !(a === at || a.contains(at))) return;
      const name = (a.getAttribute('aria-label') ?? a.textContent ?? '').replace(/\s+/g, ' ').trim().toLowerCase();
      if (!name) return;
      out.push({ index, name, route: url.pathname.slice(base.length).replace(/\/$/, ''), hash: url.hash.replace(/^#/, '') });
    });
    return out;
  }, BASE_PATH);
}

/** Whether an element holding `text` is fully in view and not covered (the sticky navbar covers the top). */
async function answerInView(page: Page, text: string): Promise<boolean> {
  return page.evaluate((text) => {
    const vh = window.innerHeight;
    const navbar = document.querySelector('nav.navbar')?.getBoundingClientRect().bottom ?? 0;
    const root = document.querySelector('main') ?? document.body;
    const hits = [...root.querySelectorAll('*')].filter(
      (el) => (el.textContent ?? '').includes(text) && ![...el.children].some((c) => (c.textContent ?? '').includes(text)),
    );
    return hits.some((el) => {
      const r = el.getBoundingClientRect();
      if (r.width === 0 || r.top < navbar - 0.5 || r.bottom > vh + 0.5) return false;
      const at = document.elementFromPoint(r.left + Math.min(r.width, 40) / 2, r.top + r.height / 2);
      return !!at && (el === at || el.contains(at) || at.contains(el));
    });
  }, text);
}

const atBottom = (page: Page): Promise<boolean> =>
  page.evaluate(() => window.scrollY + window.innerHeight >= document.documentElement.scrollHeight - 2);

async function look(page: Page, mode: Mode): Promise<void> {
  if (mode === 'keyboard') await page.keyboard.press('PageDown');
  else await page.mouse.wheel(0, Math.round((page.viewportSize()?.height ?? 700) * 0.8));
  await page.waitForTimeout(120);
}

const currentRoute = (page: Page): string => new URL(page.url()).pathname.slice(BASE_PATH.length).replace(/\/$/, '');

/** Moves focus with Tab from the top of the page (or of an open menu) to `selector`'s `nth` match; the count, or -1. */
async function tabToSelector(page: Page, selector: string, nth = 0): Promise<number> {
  await page.evaluate(() => {
    const menuOpen = !!document.querySelector('.navbar-sidebar--show');
    if (!menuOpen) {
      (document.activeElement as HTMLElement | null)?.blur();
      window.scrollTo(0, 0);
    }
  });
  for (let tabs = 1; tabs <= 250; tabs++) {
    await page.keyboard.press('Tab');
    const at = await page.evaluate(({ s, i }) => document.querySelectorAll(s)[i] === document.activeElement, { s: selector, i: nth });
    if (at) return tabs;
  }
  return -1;
}

const tabTo = (page: Page, index: number): Promise<number> => tabToSelector(page, 'a[href]', index);

/**
 * Types the task's first word into a visible search box (one action) and opens the first suggestion
 * that names one of the task's words (one action). True when that opened another page.
 */
async function search(page: Page, task: Task, mode: Mode, res: BotResult): Promise<boolean> {
  const box = page.locator('.navbar input[type="search"], .navbar input[aria-label*="search" i]').first();
  if (!(await box.count()) || !(await box.isVisible())) return false;
  const before = currentRoute(page);
  res.actions++;
  if (mode === 'keyboard') {
    const reached = await tabToSelector(page, '.navbar input[type="search"], .navbar input[aria-label*="search" i]');
    if (reached < 0) {
      res.notes.push('the search box is not reachable by Tab');
      return false;
    }
    res.keyboard_tabs += reached;
  } else {
    await box.click();
  }
  await page.keyboard.type(task.words[0], { delay: 20 });
  await page.waitForTimeout(800);
  const options = await page.evaluate(() =>
    [...document.querySelectorAll('[role="option"]')]
      .filter((o) => o.getBoundingClientRect().height > 0)
      .map((o) => (o.textContent ?? '').replace(/\s+/g, ' ').trim().toLowerCase()),
  );
  const pick = options.findIndex((o) => task.words.some((w) => o.includes(w)));
  if (pick < 0) {
    res.notes.push(`search for "${task.words[0]}" offered nothing naming the task`);
    await page.keyboard.press('Escape');
    return false;
  }
  res.actions++;
  for (let i = 0; i <= pick; i++) await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  await page.waitForFunction((old) => !location.pathname.endsWith(old === '' ? '/slot/' : old), before, { timeout: 5000 }).catch(() => undefined);
  await page.waitForTimeout(400);
  const after = currentRoute(page);
  res.path.push(`search "${task.words[0]}" -> ${after || 'landing'}${new URL(page.url()).hash}`);
  return after !== before;
}

export async function run(page: Page, task: Task, mode: Mode, viewport: string): Promise<BotResult> {
  const res: BotResult = { task: task.id, viewport, mode, done: false, actions: 0, looks: 0, steers: 0, keyboard_tabs: 0, path: [''], notes: [] };
  const visited = new Set<string>(['']);
  const fallbacks = [...FALLBACK_LINKS];
  while (res.actions <= MAX_ACTIONS) {
    const route = currentRoute(page);
    const texts = task.answers.filter(([r]) => r === route).map(([, t]) => t);
    const found = async (): Promise<boolean> => {
      for (const t of texts) if (await answerInView(page, t)) return true;
      return false;
    };
    if (texts.length) {
      // A reader on the answering page uses its table of contents first: a same-page link naming the task.
      const toc = (await visibleLinks(page)).filter((l) => l.route === route && l.hash !== '');
      let jump: SeenLink | undefined;
      for (const w of task.words) {
        jump = toc.find((l) => l.name.includes(w));
        if (jump) break;
      }
      if (jump && !(await found())) {
        res.actions++;
        if (mode === 'keyboard') {
          const tabs = await tabTo(page, jump.index);
          if (tabs >= 0) {
            res.keyboard_tabs += tabs;
            await page.keyboard.press('Enter');
          }
        } else {
          await page.locator('a[href]').nth(jump.index).click();
        }
        await page.waitForTimeout(300);
        res.path.push(`${jump.name} -> #${jump.hash}`);
      }
      // Down from where the reader is; past the end, once more from the top.
      let fromTop = false;
      for (let i = 0; i <= MAX_LOOKS_PER_PAGE * 3; i++) {
        if (await found()) {
          res.done = true;
          return res;
        }
        if (await atBottom(page)) {
          if (fromTop) break;
          fromTop = true;
          await page.evaluate(() => window.scrollTo(0, 0));
          await page.waitForTimeout(120);
          res.looks++;
          continue;
        }
        await look(page, mode);
        res.looks++;
      }
      res.notes.push(`on ${route} the answer "${texts.join('" or "')}" never came fully into view`);
    }
    let chosen: SeenLink | undefined;
    let steer = false;
    for (let i = 0; i <= MAX_LOOKS_PER_PAGE && !chosen; i++) {
      const links = (await visibleLinks(page)).filter((l) => !visited.has(l.route) && l.route !== route);
      for (const w of task.words) {
        chosen = links.find((l) => l.name.includes(w));
        if (chosen) break;
      }
      if (chosen || (await atBottom(page))) break;
      await look(page, mode);
      res.looks++;
    }
    if (!chosen) {
      await page.evaluate(() => window.scrollTo(0, 0));
      await page.waitForTimeout(150);
      if (await search(page, task, mode, res)) {
        visited.add(currentRoute(page));
        continue;
      }
      // Below the desktop width the navbar links and the sidebar sit behind the menu button.
      const toggle = page.locator('button.navbar__toggle');
      if ((await toggle.isVisible()) && !(await page.locator('.navbar-sidebar--show').count())) {
        res.actions++;
        res.steers++;
        if (mode === 'keyboard') {
          const tabs = await tabToSelector(page, 'button.navbar__toggle');
          if (tabs < 0) {
            res.notes.push('the menu button is not reachable by Tab');
            return res;
          }
          res.keyboard_tabs += tabs;
          await page.keyboard.press('Enter');
        } else {
          await toggle.click();
        }
        await page.waitForTimeout(400);
        res.path.push('menu');
        const inMenu = (await visibleLinks(page)).filter((l) => !visited.has(l.route) && l.route !== route);
        for (const w of task.words) {
          chosen = inMenu.find((l) => l.name.includes(w));
          if (chosen) break;
        }
      }
    }
    if (!chosen) {
      const links = (await visibleLinks(page)).filter((l) => !visited.has(l.route) && l.route !== route);
      while (fallbacks.length && !chosen) {
        const w = fallbacks.shift() as string;
        chosen = links.find((l) => l.name === w || l.name.startsWith(w));
      }
      steer = !!chosen;
    }
    if (!chosen) {
      res.notes.push(`no visible link on "${route || 'landing'}" names ${task.words.map((w) => `"${w}"`).join(', ')}`);
      return res;
    }
    if (steer) res.steers++;
    res.actions++;
    const before = page.url();
    if (mode === 'keyboard') {
      const tabs = await tabTo(page, chosen.index);
      if (tabs < 0) {
        res.notes.push(`"${chosen.name}" on "${route || 'landing'}" is not reachable by Tab`);
        return res;
      }
      res.keyboard_tabs += tabs;
      await page.keyboard.press('Enter');
    } else {
      await page.locator('a[href]').nth(chosen.index).click();
    }
    await page.waitForFunction((old) => location.href !== old, before, { timeout: 5000 }).catch(() => undefined);
    await page.waitForLoadState('load');
    await page.waitForTimeout(250);
    visited.add(chosen.route);
    res.path.push(`${chosen.name} -> ${currentRoute(page) || 'landing'}`);
  }
  res.notes.push(`more than ${MAX_ACTIONS} actions`);
  return res;
}
