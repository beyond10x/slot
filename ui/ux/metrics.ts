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

export const MODES: Array<BotFile['mode']> = ['pointer', 'keyboard'];
/** Name of the file gates.spec.ts writes for a scenario at a size, in `ux/out/gates`. */
export const gateFile = (scenario: string, viewport: string): string => `${scenario}@${viewport}.json`;
/** Name of the file screen-only.spec.ts writes for a task at a size in a mode, in `ux/out/bot`. */
export const botFile = (scenario: string, viewport: string, mode: BotFile['mode']): string => `${scenario}@${viewport}-${mode}.json`;

export interface ResultFiles {
  /** Paths relative to `ux/out`. */
  gates: string[];
  bots: string[];
}

/** The files one run of both specs writes at `viewports`. A test that crashes writes none. */
export function expectedFiles(scenarios: ReadonlyArray<{ name: string; task: unknown }>, viewports: readonly string[]): ResultFiles {
  const files: ResultFiles = { gates: [], bots: [] };
  for (const sc of scenarios) {
    for (const vp of viewports) {
      files.gates.push(`gates/${gateFile(sc.name, vp)}`);
      if (sc.task) for (const mode of MODES) files.bots.push(`bot/${botFile(sc.name, vp, mode)}`);
    }
  }
  return files;
}

/** The expected files that are not in `out`. */
export const missingFiles = (out: string, expected: ResultFiles): ResultFiles => ({
  gates: expected.gates.filter((f) => !existsSync(join(out, f))),
  bots: expected.bots.filter((f) => !existsSync(join(out, f))),
});

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
    // The shift of one scenario from its page load on. No rate: a scenario lasts 1 to 6 simulated seconds.
    put(g.scenario, 'layout_shift.total', Math.round(g.layout_shift.total * 1000) / 1000);
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
