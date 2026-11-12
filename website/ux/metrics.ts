// Reads the spec outputs in ux/out and reduces them to one metric table per page and per task.
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import type { Metrics } from '../../ui/ux/ratchet-lib.ts';
import type { BotResult } from './bot.ts';
import type { GateRecord } from './gates.spec.ts';
import { slug } from './pages.ts';
import { RATCHET_VIEWPORT } from './viewports.ts';

/** Score for a task the keyboard cannot finish. */
export const UNREACHABLE = 1000;

export type BotFile = BotResult & { gating: boolean };

const readAll = <T>(dir: string): T[] =>
  existsSync(dir)
    ? readdirSync(dir).filter((f) => f.endsWith('.json')).sort().map((f) => JSON.parse(readFileSync(join(dir, f), 'utf8')) as T)
    : [];

export const readGates = (out: string): GateRecord[] => readAll<GateRecord>(join(out, 'gates'));
export const readBots = (out: string): BotFile[] => readAll<BotFile>(join(out, 'bots'));

/** Metrics at the ratchet size, both themes summed: `page:<slug>` and `task:<id>` scenarios. */
export function metrics(gates: GateRecord[], bots: BotFile[]): Metrics {
  const m: Metrics = {};
  for (const g of gates.filter((g) => g.viewport === RATCHET_VIEWPORT)) {
    const k = `page:${slug(g.page)}`;
    const t = (m[k] ??= {
      'layout.doc_hscroll': 0, 'layout.past_box': 0, 'layout.scroll_boxes': 0,
      'always_visible.hidden': 0, 'axe.serious': 0, 'layout_shift.per_load': 0,
    });
    t['layout.doc_hscroll'] = Math.max(t['layout.doc_hscroll'], g.doc_hscroll);
    t['layout.past_box'] += g.past_box.length;
    t['layout.scroll_boxes'] += g.scroll_boxes.length;
    t['always_visible.hidden'] += g.hidden_always_visible.length;
    t['axe.serious'] += g.axe_serious.length;
    t['layout_shift.per_load'] = Math.max(t['layout_shift.per_load'], g.layout_shift);
  }
  for (const b of bots.filter((b) => b.viewport === RATCHET_VIEWPORT)) {
    const t = (m[`task:${b.task}`] ??= {});
    if (b.mode === 'pointer') {
      t['screen_only.failed'] = b.done ? 0 : 1;
      t['screen_only.actions'] = b.actions;
      t['screen_only.looks'] = b.looks;
      t['screen_only.steers'] = b.steers;
    } else {
      t['screen_only.keyboard_failed'] = b.done ? 0 : 1;
      t['screen_only.keyboard_tabs'] = b.done ? b.keyboard_tabs : UNREACHABLE;
    }
  }
  return m;
}
