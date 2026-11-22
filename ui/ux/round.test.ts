// The round command on output directories of its own: a result file that is missing is named and counted,
// and the report shows the round ratchet.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { test } from 'node:test';
import type { TestContext } from 'node:test';
import { fileURLToPath } from 'node:url';
import { metrics, readBots, readGates } from './metrics.ts';
import type { Baseline } from './ratchet-lib.ts';
import { SCENARIOS } from './scenarios.ts';
import { VIEWPORTS, vpName } from './viewports.ts';

const here = dirname(fileURLToPath(import.meta.url));
const TASKS = SCENARIOS.filter((sc) => sc.task).length;

interface Fixture {
  out: string;
  rounds: string;
  baseline: string;
}

/** Empty directories for one run, removed when the test ends. */
function dirs(t: TestContext): Fixture {
  const root = mkdtempSync(join(tmpdir(), 'ux-round-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  mkdirSync(join(root, 'out'));
  return { out: join(root, 'out'), rounds: join(root, 'rounds'), baseline: join(root, 'baseline.json') };
}

/** What a run of both specs at every size writes when no test crashes: 100 words, a shift of 0.01, every task done. */
function complete(t: TestContext): Fixture {
  const f = dirs(t);
  mkdirSync(join(f.out, 'gates'));
  mkdirSync(join(f.out, 'bot'));
  for (const sc of SCENARIOS) {
    for (const vp of VIEWPORTS) {
      const at = `${sc.name}@${vpName(vp)}`;
      writeFileSync(join(f.out, 'gates', `${at}.json`), JSON.stringify({
        scenario: sc.name, viewport: vpName(vp), gating: vp.gating, states: [{ moment: 'opened', probe: { words_in_view: 100 } }],
        minutes: 1 / 60, layout_shift: { total: 0.01, per_minute: 0.6 }, problems: [],
      }));
      if (!sc.task) continue;
      for (const mode of ['pointer', 'keyboard']) {
        writeFileSync(join(f.out, 'bot', `${at}-${mode}.json`), JSON.stringify({
          scenario: sc.name, viewport: vpName(vp), mode, done: true, actions: 1, looks: 1, steers: 0, time_to_act_steps: 0,
          keyboard_tabs: 4, completion_miss: false, false_complete: false, notes: [],
        }));
      }
    }
  }
  return f;
}

/** A baseline that equals what the fixture measures. */
function baselineOf(f: Fixture): Baseline {
  const scenarios: Baseline['scenarios'] = {};
  for (const [sc, keys] of Object.entries(metrics(readGates(f.out), readBots(f.out)))) {
    scenarios[sc] = Object.fromEntries(Object.entries(keys).map(([k, value]) => [k, { value, tolerance: 0 }]));
  }
  return { about: '', tolerances: {}, scenarios };
}

interface Run {
  code: number | null;
  report: string;
  trendRow: string;
  json: { problems: string[]; missing?: { gates: string[]; bots: string[] }; trend?: Record<string, number> };
}

function round(f: Fixture): Run {
  const p = spawnSync(process.execPath, ['--disable-warning=ExperimentalWarning', join(here, 'round.ts'), 'R-01'], {
    env: { ...process.env, UX_OUT: f.out, UX_ROUNDS: f.rounds, UX_BASELINE: f.baseline }, encoding: 'utf8',
  });
  const dir = join(f.rounds, 'R-01');
  assert.ok(existsSync(join(dir, 'report.md')), `no report written: ${p.stderr}`);
  return {
    code: p.status,
    report: readFileSync(join(dir, 'report.md'), 'utf8'),
    trendRow: readFileSync(join(f.rounds, 'trend.md'), 'utf8').split('\n').find((l) => l.startsWith('| R-01 |')) ?? '',
    json: JSON.parse(readFileSync(join(dir, 'round.json'), 'utf8')) as Run['json'],
  };
}

/** The lines of one `## heading` section of the report. */
function section(report: string, heading: string): string[] {
  const lines = report.split('\n');
  const at = lines.indexOf(`## ${heading}`);
  assert.notEqual(at, -1, `the report has no section "${heading}"`);
  const rest = lines.slice(at + 1);
  const end = rest.findIndex((l) => l.startsWith('## '));
  return (end === -1 ? rest : rest.slice(0, end)).filter((l) => l.trim());
}

test('a complete output directory: nothing is missing and the round exits 0', (t) => {
  const r = round(complete(t));
  assert.equal(r.code, 0);
  assert.deepEqual(section(r.report, 'Missing result files').slice(-1), ['None.']);
  assert.deepEqual(r.json.missing, { gates: [], bots: [] });
  assert.equal(r.trendRow, '| R-01 | 0 | 0 | 0 |');
});

test('a gate file that a crashed test did not write is listed, counted as a gate problem, and the round exits 1', (t) => {
  const f = complete(t);
  rmSync(join(f.out, 'gates', 'crowded@1280x720.json'));
  const r = round(f);
  assert.equal(r.code, 1);
  assert.deepEqual(section(r.report, 'Missing result files').filter((l) => l.startsWith('- ')), ['- gates/crowded@1280x720.json']);
  assert.deepEqual(r.json.missing, { gates: ['gates/crowded@1280x720.json'], bots: [] });
  assert.equal(r.json.problems.length, 1);
  assert.equal(r.trendRow, '| R-01 | 1 | 0 | 0 |');
});

test('a screen-only file that is missing counts as a failure of its mode', (t) => {
  const f = complete(t);
  rmSync(join(f.out, 'bot', 'busy-failed@1366x768-pointer.json'));
  rmSync(join(f.out, 'bot', 'busy-failed@960x1080-keyboard.json'));
  const r = round(f);
  assert.equal(r.code, 1);
  assert.deepEqual(section(r.report, 'Missing result files').filter((l) => l.startsWith('- ')), [
    '- bot/busy-failed@1366x768-pointer.json', '- bot/busy-failed@960x1080-keyboard.json',
  ]);
  assert.deepEqual(r.json.trend, { gate_problems: 0, screen_only_failures: 1, keyboard_failures: 1 });
  assert.equal(r.trendRow, '| R-01 | 0 | 1 | 1 |');
});

test('an empty output directory: every expected file is missing, none reads as a pass', (t) => {
  const r = round(dirs(t));
  const gates = SCENARIOS.length * VIEWPORTS.length;
  const perMode = TASKS * VIEWPORTS.length;
  assert.equal(r.code, 1);
  assert.equal(section(r.report, 'Missing result files').filter((l) => l.startsWith('- ')).length, gates + 2 * perMode);
  assert.equal(r.trendRow, `| R-01 | ${gates} | ${perMode} | ${perMode} |`);
});

test('the report shows the round ratchet: at the baseline, and every failing line, host-dependent keys included', (t) => {
  const f = complete(t);
  const base = baselineOf(f);
  writeFileSync(f.baseline, JSON.stringify(base));
  assert.deepEqual(section(round(f).report, 'Round ratchet').slice(-1), ['At or better than the baseline.']);
  base.scenarios.crowded['reading_load.max'] = { value: 50, tolerance: 5 };
  delete base.scenarios.idle['layout_shift.total'];
  writeFileSync(f.baseline, JSON.stringify(base));
  const r = round(f);
  assert.deepEqual(section(r.report, 'Round ratchet').filter((l) => l.startsWith('- ')), [
    '- crowded reading_load.max: 100 is worse than the baseline 50 (tolerance 5)',
    '- idle layout_shift.total: measured but not in the baseline',
  ]);
  assert.equal(r.code, 0, 'the ratchet command, not the report, fails the round task');
});

test('the round task ends with the round ratchet', () => {
  const taskfile = readFileSync(join(here, '..', '..', 'Taskfile.yml'), 'utf8');
  const task = /\n  ux-round:\n([\s\S]*?)\n\n/.exec(taskfile)?.[1] ?? '';
  const cmds = task.split('\n').filter((l) => l.startsWith('      - ')).map((l) => l.slice(8));
  assert.equal(cmds.at(-1), 'node ux/ratchet.ts --round');
  assert.equal(cmds.at(-2), 'node ux/round.ts {{.CLI_ARGS}}');
});
