// Reads the spec outputs in ux/out and reduces them to one metric table per scenario.
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import type { Metrics } from './ratchet-lib.ts';

export const RATCHET_VIEWPORT = '1366x768';
/** Score for a task the keyboard cannot reach. */
export const UNREACHABLE = 1000;

export interface GateFile {
  scenario: string;
  viewport: string;
  gating: boolean;
  states: Array<{ moment: string; probe: { words_in_view: number } }>;
  layout_shift: { total: number; per_minute: number };
  /** Simulated minutes of the scenario. */
  minutes: number;
  problems: string[];
}

export interface BotFile {
  scenario: string;
  viewport: string;
  mode: 'pointer' | 'keyboard';
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

const readAll = <T>(dir: string): T[] =>
  existsSync(dir)
    ? readdirSync(dir).filter((f) => f.endsWith('.json')).map((f) => JSON.parse(readFileSync(join(dir, f), 'utf8')) as T)
    : [];

export const readGates = (out: string): GateFile[] => readAll<GateFile>(join(out, 'gates'));
export const readBots = (out: string): BotFile[] => readAll<BotFile>(join(out, 'bot'));

function median(xs: number[]): number {
  const s = [...xs].sort((a, b) => a - b);
  const m = Math.floor(s.length / 2);
  return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2;
}

export function metrics(gates: GateFile[], bots: BotFile[]): Metrics {
  const out: Metrics = {};
  const put = (sc: string, k: string, v: number): void => {
    out[sc] ??= {};
    out[sc][k] = v;
  };
  for (const g of gates) {
    if (g.gating) put(g.scenario, 'gates.problems', (out[g.scenario]?.['gates.problems'] ?? 0) + g.problems.length);
    if (g.viewport !== RATCHET_VIEWPORT) continue;
    const words = g.states.map((s) => s.probe.words_in_view);
    put(g.scenario, 'reading_load.max', Math.max(...words));
    put(g.scenario, 'reading_load.median', median(words));
    put(g.scenario, 'layout_shift.per_minute', Math.round(g.layout_shift.total / Math.max(1, g.minutes) * 1000) / 1000);
  }
  for (const b of bots) {
    if (b.viewport !== RATCHET_VIEWPORT) continue;
    const failed = !b.done || b.completion_miss || b.false_complete ? 1 : 0;
    if (b.mode === 'keyboard') {
      put(b.scenario, 'screen_only.keyboard_tabs', b.done ? b.keyboard_tabs : UNREACHABLE);
      put(b.scenario, 'screen_only.keyboard_failed', failed);
      continue;
    }
    put(b.scenario, 'screen_only.actions', b.actions);
    put(b.scenario, 'screen_only.looks', b.looks);
    put(b.scenario, 'screen_only.steers', b.steers);
    put(b.scenario, 'screen_only.time_to_act_steps.max', b.time_to_act_steps);
    put(b.scenario, 'actions_per_fix.max', b.actions);
    put(b.scenario, 'screen_only.failed', failed);
  }
  return out;
}
