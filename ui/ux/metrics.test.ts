import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { metrics } from './metrics.ts';
import type { GateFile } from './metrics.ts';
import type { Baseline } from './ratchet-lib.ts';

const here = dirname(fileURLToPath(import.meta.url));

const gate = (seconds: number): GateFile => ({
  scenario: 's', viewport: '1366x768', gating: true, states: [{ moment: 'opened', probe: { words_in_view: 100 } }],
  minutes: seconds / 60, layout_shift: { total: 0.0347, per_minute: 0.0347 / (seconds / 60) }, problems: [],
});

test('layout shift is the total of the scenario under the key layout_shift.total, whatever the scenario lasts', () => {
  for (const seconds of [1, 6, 60, 600]) {
    const m = metrics([gate(seconds)], []).s;
    assert.equal(m['layout_shift.total'], 0.035, `${seconds} s`);
    assert.equal('layout_shift.per_minute' in m, false, 'no key that names a rate');
  }
});

test('the baseline holds layout_shift.total for every scenario and no key that names a rate', () => {
  const base = JSON.parse(readFileSync(join(here, 'baseline.json'), 'utf8')) as Baseline;
  assert.equal(JSON.stringify(base).includes('per_minute'), false);
  assert.equal(base.tolerances['layout_shift.total'], '0.01');
  for (const [sc, keys] of Object.entries(base.scenarios)) {
    if ('reading_load.max' in keys) assert.equal(keys['layout_shift.total']?.tolerance, 0.01, sc);
  }
});
