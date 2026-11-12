// Measurements taken in a docs page: layout contract, always-visible elements, accessibility.
import AxeBuilder from '@axe-core/playwright';
import type { Page } from '@playwright/test';
import { DESKTOP_MIN_WIDTH } from './viewports.ts';

/** DejaVu Sans on every host: the workstation's sans-serif is narrower than CI's (ui/ux UXF-016). */
export const FONT_CSS =
  "html, body, button, input, select, .navbar, .menu, .markdown { font-family: 'DejaVu Sans', sans-serif !important; }";

/** Elements a reader needs at every scroll position on a desktop-sized window. */
export const ALWAYS_VISIBLE: Array<[string, string]> = [
  ['navbar', 'nav.navbar'],
  ['search', '.navbar [role="search"], .navbar input[type="search"], .navbar input[aria-label*="search" i], .navbar .DocSearch-Button'],
  ['theme toggle', '.navbar button[title*="mode" i], .navbar button[aria-label*="mode" i]'],
];

/** Below the desktop width: the controls that replace the navbar links and the sidebar. */
export const ALWAYS_VISIBLE_NARROW: Array<[string, string]> = [
  ['navbar', 'nav.navbar'],
  ['menu button', 'button.navbar__toggle'],
  ['search', '.navbar input[aria-label*="search" i], .navbar input[type="search"]'],
];

/** On doc pages only: the sidebar menu. */
export const SIDEBAR: [string, string] = ['sidebar', 'aside.theme-doc-sidebar-container nav.menu'];

export interface Probe {
  doc_hscroll: number;
  /** Elements whose content runs past their own box, without scrolling or clipping it. */
  past_box: string[];
  /** Boxes the reader scrolls sideways inside (code blocks, tables). Measured, not gated. */
  scroll_boxes: string[];
  hidden_always_visible: string[];
  axe_serious: string[];
}

export async function layout(page: Page, docPage: boolean): Promise<Omit<Probe, 'axe_serious'>> {
  const always = [...ALWAYS_VISIBLE];
  if (docPage) always.push(SIDEBAR);
  return page.evaluate(
    ({ wide, narrow, desktop }) => {
      const vw = document.documentElement.clientWidth;
      const vh = window.innerHeight;
      const name = (el: Element): string => {
        const id = el.id ? `#${el.id}` : '';
        const cls = [...el.classList].filter((c) => !/_[a-zA-Z0-9]{4,}$/.test(c)).slice(0, 2);
        return `${el.tagName.toLowerCase()}${id}${cls.length ? `.${cls.join('.')}` : ''}`;
      };
      const pastBox: string[] = [];
      const scrollBoxes: string[] = [];
      for (const el of document.body.querySelectorAll('*')) {
        const cs = getComputedStyle(el);
        if (cs.display === 'none' || cs.visibility === 'hidden' || cs.display === 'inline' || cs.display === 'contents') continue;
        if (el.closest('svg') && el.tagName.toLowerCase() !== 'svg') continue;
        const he = el as HTMLElement;
        if (he.clientWidth === 0) continue;
        const r = el.getBoundingClientRect();
        if (cs.overflowX === 'auto' || cs.overflowX === 'scroll') {
          if (he.scrollWidth > he.clientWidth + 1 && r.width > 0) scrollBoxes.push(`${name(el)} by ${he.scrollWidth - he.clientWidth}px`);
          continue;
        }
        if (cs.overflowX !== 'visible') continue;
        // Content of a box the reader scrolls sideways (a code block) is counted in scroll_boxes.
        let scrolled = false;
        for (let a = el.parentElement; a && !scrolled; a = a.parentElement) {
          const ox = getComputedStyle(a).overflowX;
          scrolled = ox === 'auto' || ox === 'scroll';
        }
        if (scrolled) continue;
        for (const child of el.children) {
          const ccs = getComputedStyle(child);
          if (ccs.position === 'fixed' || ccs.position === 'absolute' || ccs.display === 'none') continue;
          const c = child.getBoundingClientRect();
          // A grid row's negative margin (Infima `.row`) reaches past its parent by design.
          const allowed = Math.max(0, -parseFloat(ccs.marginRight) || 0);
          if (c.width > 0 && c.right > r.right + allowed + 1) {
            pastBox.push(`${name(el)} > ${name(child)} by ${Math.round(c.right - r.right)}px`);
            break;
          }
        }
      }
      const hidden: string[] = [];
      for (const [label, sel] of vw >= desktop ? wide : narrow) {
        const el = document.querySelector(sel);
        if (!el) {
          hidden.push(`${label}: missing`);
          continue;
        }
        const r = el.getBoundingClientRect();
        // The sidebar is taller than the window: its top must be in view, not all of it.
        const bottom = label === 'sidebar' ? Math.min(r.bottom, r.top + 40) : r.bottom;
        if (r.width === 0 || r.left < 0 || r.top < 0 || r.right > vw + 0.5 || bottom > vh + 0.5) {
          hidden.push(`${label}: outside the viewport`);
          continue;
        }
        const at = document.elementFromPoint(r.left + r.width / 2, r.top + Math.min(r.height, 40) / 2);
        if (!at || !(el === at || el.contains(at) || at.contains(el))) hidden.push(`${label}: covered`);
      }
      return {
        doc_hscroll: Math.max(0, document.documentElement.scrollWidth - vw),
        past_box: [...new Set(pastBox)].slice(0, 10),
        scroll_boxes: scrollBoxes.slice(0, 20),
        hidden_always_visible: hidden,
      };
    },
    { wide: always, narrow: ALWAYS_VISIBLE_NARROW, desktop: DESKTOP_MIN_WIDTH },
  );
}

export async function axeSerious(page: Page): Promise<string[]> {
  const axe = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa']).analyze();
  return axe.violations
    .filter((v) => v.impact === 'serious' || v.impact === 'critical')
    .map((v) => `${v.id} (${v.nodes.length}): ${v.nodes.slice(0, 2).map((n) => n.target.join(' ')).join(', ')}`);
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

export const layoutShift = (page: Page): Promise<number> => page.evaluate(() => (window as unknown as { __uxCls: number }).__uxCls);

/** Opens `path` in `theme`, with the pinned font, and waits for the page to settle. */
export async function openPage(page: Page, path: string, theme: 'light' | 'dark'): Promise<void> {
  await page.addInitScript((t) => {
    try {
      localStorage.setItem('theme', t);
    } catch {
      /* storage blocked */
    }
  }, theme);
  await page.addInitScript(clsObserver);
  await page.addInitScript((css) => {
    document.addEventListener('DOMContentLoaded', () => {
      const st = document.createElement('style');
      st.textContent = css;
      document.head.appendChild(st);
    });
  }, FONT_CSS);
  await page.emulateMedia({ reducedMotion: 'reduce', colorScheme: theme });
  await page.goto(path, { waitUntil: 'load' });
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(300);
}
