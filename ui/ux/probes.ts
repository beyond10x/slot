// Measurements taken in the page at one state: layout contract, always-visible elements, reading load.
import AxeBuilder from '@axe-core/playwright';
import type { Page } from '@playwright/test';

/** Elements that must be fully in the viewport and not covered at every state. */
export const ALWAYS_VISIBLE: Array<[string, string]> = [
  ['slots in use', '[data-ux-region="slots"]'],
  ['headroom control', '[data-ux-region="headroom"]'],
  ['queue length', '[data-ux-region="queue"]'],
  ['connection state', '[data-ux-region="connection"]'],
];

/** Fallbacks for a page without region marks: the header and its slider. */
const LEGACY: Array<[string, string]> = [
  ['header', 'header'],
  ['headroom control', 'header input[type=range]'],
];

/** Elements allowed to scroll on their own: the output pane's text. */
const SCROLL_ALLOWED = ['pre'];

export interface Probe {
  doc_hscroll: number;
  /** Innermost elements running past the right edge when the page scrolls sideways. */
  past_viewport: string[];
  /** Children wider than their panel (main column, section, output pane bar). */
  past_box: string[];
  scrollers: string[];
  clipped_without_full_value: string[];
  hidden_always_visible: string[];
  words_in_view: number;
  axe_serious: string[];
}

export async function probe(page: Page): Promise<Probe> {
  const marked = (await page.locator('[data-ux-region]').count()) > 0;
  const layout = await page.evaluate(
    ({ always, allowed }) => {
      const vw = window.innerWidth;
      const vh = window.innerHeight;
      const name = (el: Element): string => {
        const id = el.id ? `#${el.id}` : '';
        const cls = el.classList.length ? `.${[...el.classList].join('.')}` : '';
        return `${el.tagName.toLowerCase()}${id}${cls}`;
      };
      const scrollers: string[] = [];
      const clipped: string[] = [];
      for (const el of document.body.querySelectorAll('*')) {
        const cs = getComputedStyle(el);
        if (cs.display === 'none' || cs.visibility === 'hidden') continue;
        const he = el as HTMLElement;
        const scrollsX = (cs.overflowX === 'auto' || cs.overflowX === 'scroll') && he.scrollWidth > he.clientWidth + 1;
        const scrollsY = (cs.overflowY === 'auto' || cs.overflowY === 'scroll') && he.scrollHeight > he.clientHeight + 1;
        if ((scrollsX || scrollsY) && !allowed.includes(el.tagName.toLowerCase())) scrollers.push(name(el));
        if (cs.textOverflow === 'ellipsis' && he.scrollWidth > he.clientWidth + 1) {
          const full = (he.textContent ?? '').trim();
          const title = he.getAttribute('title') ?? '';
          if (!title.includes(full)) clipped.push(`${name(el)}: ${full.slice(0, 40)}`);
        }
      }
      const hidden: string[] = [];
      for (const [label, sel] of always) {
        const el = document.querySelector(sel);
        if (!el) {
          hidden.push(`${label}: missing`);
          continue;
        }
        const r = el.getBoundingClientRect();
        if (r.left < 0 || r.top < 0 || r.right > vw + 0.5 || r.bottom > vh + 0.5 || r.width === 0) {
          hidden.push(`${label}: outside the viewport`);
          continue;
        }
        const at = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
        if (!at || !(el === at || el.contains(at))) hidden.push(`${label}: covered`);
      }
      let words = 0;
      const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
      const range = document.createRange();
      for (let n = walker.nextNode(); n; n = walker.nextNode()) {
        const text = n.textContent?.trim() ?? '';
        if (!text) continue;
        const p = n.parentElement;
        if (!p || p.closest('[hidden]')) continue;
        const cs = getComputedStyle(p);
        if (cs.visibility === 'hidden' || cs.display === 'none') continue;
        range.selectNodeContents(n);
        const r = range.getBoundingClientRect();
        if (r.width === 0 || r.bottom <= 0 || r.top >= vh || r.right <= 0 || r.left >= vw) continue;
        words += text.split(/\s+/).length;
      }
      // Content wider than its panel: hidden under the output pane or cut, though the page does not scroll.
      const pastBox: string[] = [];
      for (const box of document.querySelectorAll('main > div, main section, aside .bar')) {
        const b = box.getBoundingClientRect();
        for (const child of box.children) {
          const c = child.getBoundingClientRect();
          if (c.width > 0 && c.right > b.right + 1) {
            const label = child.previousElementSibling?.tagName === 'H2' ? ` (${child.previousElementSibling.textContent?.slice(0, 24)})` : '';
            pastBox.push(`${name(box)} > ${name(child)}${label} by ${Math.round(c.right - b.right)}px`);
          }
        }
      }
      const past: string[] = [];
      if (document.documentElement.scrollWidth > vw) {
        for (const el of document.body.querySelectorAll('*')) {
          if (el.getBoundingClientRect().right <= vw + 0.5) continue;
          const kids = [...el.children].some((c) => c.getBoundingClientRect().right > vw + 0.5);
          if (!kids) past.push(`${name(el.parentElement ?? el)} > ${name(el)}`);
          if (past.length >= 3) break;
        }
      }
      return {
        doc_hscroll: Math.max(0, document.documentElement.scrollWidth - vw),
        past_viewport: past,
        past_box: pastBox,
        scrollers,
        clipped_without_full_value: clipped,
        hidden_always_visible: hidden,
        words_in_view: words,
      };
    },
    { always: marked ? ALWAYS_VISIBLE : LEGACY, allowed: SCROLL_ALLOWED },
  );
  const axe = await new AxeBuilder({ page })
    .withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa'])
    .analyze();
  const axe_serious = axe.violations
    .filter((v) => v.impact === 'serious' || v.impact === 'critical')
    .map((v) => `${v.id} (${v.nodes.length}): ${v.nodes.slice(0, 2).map((n) => n.target.join(' ')).join(', ')}`);
  return { ...layout, axe_serious };
}

/** Installed before page scripts: sums layout shifts not caused by input. */
export function clsObserver(): void {
  const w = window as unknown as { __uxCls: number };
  w.__uxCls = 0;
  new PerformanceObserver((list) => {
    for (const e of list.getEntries() as Array<PerformanceEntry & { value: number; hadRecentInput: boolean }>) {
      if (!e.hadRecentInput) w.__uxCls += e.value;
    }
  }).observe({ type: 'layout-shift', buffered: true });
}

export const layoutShift = (page: Page): Promise<number> =>
  page.evaluate(() => (window as unknown as { __uxCls: number }).__uxCls);
