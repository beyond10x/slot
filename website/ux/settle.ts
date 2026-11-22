// What a docs test waits for in place of a fixed time: the page hydrated, no script or request of
// the page in flight, no scroll or transition running, and a frame that changed nothing in the
// document. A fixed wait is too short on a loaded host (the reader then counts Tab stops and looks
// on a page that is not finished) and too long on an idle one.
import type { Page } from '@playwright/test';

/** The most frames a wait takes before the test goes on: 2 s of frames at 60 per second. */
export const SETTLE_FRAMES = 120;

interface Ux {
  settle(frames: number, scroll: boolean): Promise<number>;
}

/** Installed before any page script: counts what is in flight and what changed, `window.__uxDocs.settle` waits. */
export function settleScript(): void {
  // Requests and scripts in flight. One ends a task after it loaded, so the page has used it by then.
  let inFlight = 0;
  const ended = (): void => {
    setTimeout(() => {
      inFlight--;
    }, 0);
  };
  const fetchPage = window.fetch.bind(window);
  window.fetch = (...args: Parameters<typeof fetch>) => {
    inFlight++;
    const res = fetchPage(...args);
    res.then((r) => r.clone().arrayBuffer()).then(ended, ended);
    return res;
  };
  let changes = 0;
  new MutationObserver((list) => {
    changes += list.length;
    for (const m of list) {
      for (const n of m.addedNodes) {
        // A page chunk: the site loads the code of a page as a script when a link to it is taken.
        if (!(n instanceof HTMLScriptElement) || !n.src) continue;
        inFlight++;
        let once = false;
        const done = (): void => {
          if (!once) ended();
          once = true;
        };
        n.addEventListener('load', done);
        n.addEventListener('error', done);
      }
    }
  }).observe(document, { subtree: true, childList: true, attributes: true, characterData: true });
  // Scrolls of the window: PageDown is drawn over several frames, `scrollend` is its last.
  let scrolls = 0;
  let seen = 0;
  let scrolling = false;
  document.addEventListener('scroll', () => {
    scrolls++;
    scrolling = true;
  });
  document.addEventListener('scrollend', () => {
    scrolling = false;
  });
  const frame = (): Promise<void> => new Promise((ok) => requestAnimationFrame(() => ok()));
  const moving = (): boolean =>
    document.getAnimations().some((a) => a.playState === 'running' && a.effect?.getComputedTiming().endTime !== Infinity);
  const ux: Ux = {
    async settle(frames: number, scroll: boolean): Promise<number> {
      let n = 0;
      // A scroll the test started shows as a scroll event first; without one after half the frames, none came.
      if (scroll) {
        while (n < frames / 2 && scrolls === seen) {
          await frame();
          n++;
        }
      }
      await frame();
      n++;
      let still = 0;
      while (n < frames) {
        const before = changes;
        const y = window.scrollY;
        await frame();
        n++;
        still = window.scrollY === y ? still + 1 : 0;
        // A scroll with no `scrollend` counts as over after ten frames at one position.
        if (changes === before && inFlight === 0 && still > 0 && (!scrolling || still >= 10) && !moving()) break;
      }
      scrolling = false;
      seen = scrolls;
      return n;
    },
  };
  (window as unknown as { __uxDocs: Ux }).__uxDocs = ux;
}

/**
 * Waits until the page is at rest; the frames it took, `SETTLE_FRAMES` when it did not come to rest.
 * With `scroll` it first waits for the scroll the test just started.
 */
export const settle = (page: Page, scroll = false): Promise<number> =>
  page.evaluate(([n, s]) => (window as unknown as { __uxDocs: Ux }).__uxDocs.settle(n, s), [SETTLE_FRAMES, scroll] as const);

/** Waits until the site's code took over the served HTML: code blocks have their buttons only then. */
export async function hydrated(page: Page): Promise<void> {
  await page.waitForFunction(() => document.documentElement.getAttribute('data-has-hydrated') === 'true');
}

/** What tells one rendered page from another: its title and first heading. */
export const pageSignature = (page: Page): Promise<string> =>
  page.evaluate(() => `${document.title}\n${document.querySelector('h1')?.textContent ?? ''}`);

/**
 * After a link was taken from the page at `url` with `signature`: waits until the address changed and
 * the other page is drawn (the site changes the address first and draws when the page's code
 * arrived), then until it is at rest. Goes on after 5 s when the link led nowhere.
 */
export async function arrived(page: Page, url: string, signature: string): Promise<void> {
  await page
    .waitForFunction(
      ([url, signature]) => location.href !== url && `${document.title}\n${document.querySelector('h1')?.textContent ?? ''}` !== signature,
      [url, signature] as const,
      { timeout: 5000 },
    )
    .catch(() => undefined);
  await page.waitForLoadState('load');
  await hydrated(page);
  await settle(page);
}
