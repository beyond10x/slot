// Ratchet gate: node ux/ratchet.ts [--round] [--update]. Exit 1 on a worse value, an untaken improvement,
// a missing value, or a baseline worse than main's without a decision id. Without --round, the keys
// that depend on the host's fonts (reading load, layout shift) are not compared: every merge runs
// on CI's host, a round on one host for all of its measurements.
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { metrics, readBots, readGates } from './metrics.ts';
import { compare, hardFailures, hostDependent } from './ratchet-lib.ts';
import type { Baseline } from './ratchet-lib.ts';

const here = dirname(fileURLToPath(import.meta.url));
const file = join(here, 'baseline.json');
const update = process.argv.includes('--update');
const round = process.argv.includes('--round') || !existsSync(join(here, 'baseline.json'));

function mainBaseline(): Baseline | undefined {
  for (const ref of ['origin/main', 'main']) {
    try {
      return JSON.parse(execFileSync('git', ['show', `${ref}:ui/ux/baseline.json`], { cwd: here, encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] })) as Baseline;
    } catch {
      /* not on that ref, or the ref is not fetched */
    }
  }
  return undefined;
}

const EMPTY: Baseline = {
  about: 'UX ratchet of the slot web UI: lower is better for every key. Written by ux/ratchet.ts --update.',
  tolerances: { counts: '0', 'reading_load.*': '10 % (fonts differ between hosts)', 'layout_shift.per_minute': '0.01' },
  scenarios: {},
};
const base = existsSync(file) ? (JSON.parse(readFileSync(file, 'utf8')) as Baseline) : EMPTY;
const main = mainBaseline();
if (!main) console.log('ratchet: no baseline on main to compare with');
const current = metrics(readGates(join(here, 'out')), readBots(join(here, 'out')));
const outcome = compare(base, current, main, round ? () => false : hostDependent);
if (update) {
  writeFileSync(file, JSON.stringify(outcome.next, null, 2) + '\n');
  const left = hardFailures(outcome);
  for (const f of outcome.failures) console.log(`${left.includes(f) ? 'FAIL' : 'updated'}: ${f}`);
  process.exit(left.length ? 1 : 0);
}
for (const f of outcome.failures) console.log(`FAIL: ${f}`);
if (!outcome.failures.length) console.log(`ratchet: ${Object.keys(base.scenarios).length} scenarios at or better than the baseline`);
process.exit(outcome.failures.length ? 1 : 0);
