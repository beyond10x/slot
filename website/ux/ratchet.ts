// Docs ratchet gate: node ux/ratchet.ts [--round] [--update]. The same rules as ui/ux/ratchet.ts and
// its comparison code: exit 1 on a worse value, an untaken improvement, a missing value, or a
// baseline worse than main's without a decision id. Layout shift depends on the host and is compared
// only in a round.
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { compare, hardFailures, hostDependent } from '../../ui/ux/ratchet-lib.ts';
import type { Baseline } from '../../ui/ux/ratchet-lib.ts';
import { metrics, readBots, readGates } from './metrics.ts';

const here = dirname(fileURLToPath(import.meta.url));
const file = join(here, 'baseline.json');
const update = process.argv.includes('--update');
const round = process.argv.includes('--round');

function mainBaseline(): Baseline | undefined {
  for (const ref of ['origin/main', 'main']) {
    try {
      return JSON.parse(execFileSync('git', ['show', `${ref}:website/ux/baseline.json`], { cwd: here, encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] })) as Baseline;
    } catch {
      /* not on that ref, or the ref is not fetched */
    }
  }
  return undefined;
}

const EMPTY: Baseline = {
  about: 'UX ratchet of the slot docs site at 1366x768, light and dark summed: lower is better for every key. Written by ux/ratchet.ts --update.',
  tolerances: {
    counts: '0',
    'layout_shift.per_load': '0, compared only in a round (host fonts)',
    'screen_only.keyboard_tabs': '0, compared only in a round (moves whenever a page gains a heading or link)',
  },
  scenarios: {},
};
const base = existsSync(file) ? (JSON.parse(readFileSync(file, 'utf8')) as Baseline) : EMPTY;
const main = mainBaseline();
if (!main) console.log('ratchet: no docs baseline on main to compare with');
const current = metrics(readGates(join(here, 'out')), readBots(join(here, 'out')));
const roundOnly = (key: string): boolean => hostDependent(key) || key === 'screen_only.keyboard_tabs';
const outcome = compare(base, current, main, round ? () => false : roundOnly);
if (update) {
  writeFileSync(file, JSON.stringify(outcome.next, null, 2) + '\n');
  const left = hardFailures(outcome);
  for (const f of outcome.failures) console.log(`${left.includes(f) ? 'FAIL' : 'updated'}: ${f}`);
  process.exit(left.length ? 1 : 0);
}
for (const f of outcome.failures) console.log(`FAIL: ${f}`);
if (!outcome.failures.length) console.log(`ratchet: ${Object.keys(base.scenarios).length} docs scenarios at or better than the baseline`);
process.exit(outcome.failures.length ? 1 : 0);
