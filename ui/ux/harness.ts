// Drives the built page through a scenario with no daemon: fixed responses, a fake EventSource, a fixed clock.
import type { Page } from '@playwright/test';
import type { Scenario } from './scenarios.ts';
import { EMPTY_USAGE, T0 } from './scenarios.ts';

/** DejaVu Sans: installed with Playwright's browser dependencies, and wider than Noto Sans. */
export const FONT_CSS = "body, button, input, select { font-family: 'DejaVu Sans', sans-serif !important; }";

interface Outputs {
  [id: string]: { chunks: Array<{ t: string; b: string }>; end: string };
}

/** Installed before any page script: `EventSource` is replaced; `window.__ux.push` delivers to open streams. */
function fakeEventSource(outputs: Outputs): void {
  type Listener = (ev: MessageEvent<string>) => void;
  const sources: FakeES[] = [];
  class FakeES extends EventTarget {
    static readonly CONNECTING = 0;
    static readonly OPEN = 1;
    static readonly CLOSED = 2;
    url: string;
    readyState = 0;
    onopen: ((ev: Event) => void) | null = null;
    onmessage: Listener | null = null;
    onerror: ((ev: Event) => void) | null = null;
    constructor(url: string | URL) {
      super();
      this.url = String(url);
      sources.push(this);
      setTimeout(() => {
        this.readyState = 1;
        this.onopen?.(new Event('open'));
        const m = /jobs\/(\d+)\/output$/.exec(this.url);
        const out = m ? outputs[m[1]] : undefined;
        if (out) {
          for (const c of out.chunks) this.emit('message', JSON.stringify(c));
          if (out.end !== '') this.emit('end', JSON.stringify({ msg: out.end }));
        }
      }, 0);
    }
    emit(type: string, data: string): void {
      if (this.readyState !== 1) return;
      const ev = new MessageEvent<string>(type, { data });
      if (type === 'message') this.onmessage?.(ev);
      this.dispatchEvent(ev);
    }
    close(): void {
      this.readyState = 2;
    }
  }
  const w = window as unknown as { EventSource: unknown; __ux: unknown };
  w.EventSource = FakeES;
  w.__ux = {
    push(suffix: string, data: string): void {
      for (const src of sources) if (src.url.endsWith(suffix)) src.emit('message', data);
    },
  };
}

export interface Opened {
  /** Bodies of every `PUT api/limits`. */
  limitPuts: unknown[];
  /** Simulated seconds since the scenario started. */
  elapsed: () => number;
  /** Applies the next step: its events, then the clock. Returns false after the last. */
  step: () => Promise<boolean>;
  /** Index of the step applied last; -1 before the first. */
  index: () => number;
}

/** Opens the built page on `base` with every API answered from `sc`. */
export async function open(page: Page, base: string, sc: Scenario): Promise<Opened> {
  const outputs: Outputs = {};
  for (const [id, o] of Object.entries(sc.output)) outputs[id] = o;
  await page.addInitScript(fakeEventSource, outputs);
  await page.clock.setFixedTime(T0);
  await page.emulateMedia({ reducedMotion: 'reduce' });
  const limitPuts: unknown[] = [];
  await page.route('**/api/state', (r) => r.fulfill({ json: sc.state }));
  await page.route('**/api/history', (r) => r.fulfill({ json: sc.history }));
  await page.route('**/api/usage**', (r) => r.fulfill({ json: sc.usage ?? EMPTY_USAGE }));
  await page.route('**/api/limits', async (r) => {
    const body = r.request().postDataJSON() as { headroom_percent: number };
    limitPuts.push(body);
    if (sc.task?.kind === 'set-headroom' && sc.task.fails) {
      return r.fulfill({ status: 200, contentType: 'text/html', body: '<!doctype html><title>slot</title>' });
    }
    const h = body.headroom_percent;
    const tokens = sc.state.tokens;
    return r.fulfill({
      json: {
        headroom_percent: h, effective_tokens: Math.max(1, Math.round((tokens * (100 - h)) / 100)),
        tokens, cpu_quota_percent: h ? (100 - h) * sc.state.cores : null, systemd: true,
      },
    });
  });
  // One wide font on every host: the workstation's sans-serif is narrower than CI's, and a layout
  // that fits only the narrower one passed here and failed there (UXF-016).
  await page.addInitScript((css) => {
    document.addEventListener('DOMContentLoaded', () => {
      const st = document.createElement('style');
      st.textContent = css;
      document.head.appendChild(st);
    });
  }, FONT_CSS);
  await page.goto(base);
  await page.waitForFunction(() => document.querySelector('header h1')?.textContent?.includes('workstation'));
  await page.locator('main').waitFor();
  let i = -1;
  let elapsed = 0;
  return {
    limitPuts,
    elapsed: () => elapsed,
    index: () => i,
    step: async () => {
      if (i + 1 >= sc.steps.length) return false;
      i++;
      const st = sc.steps[i];
      for (const ev of st.events) {
        await page.evaluate((d) => (window as unknown as { __ux: { push(s: string, d: string): void } }).__ux.push('api/events', d), JSON.stringify(ev));
      }
      elapsed += st.dt_s;
      await page.clock.setFixedTime(T0 + elapsed * 1000);
      await page.waitForTimeout(1100);
      return true;
    },
  };
}
