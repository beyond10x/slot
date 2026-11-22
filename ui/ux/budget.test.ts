// The test budget of the gates: the runner stops a test at 60 s and names the slowest tests.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import config from '../playwright.config.ts';
import { slowest } from './slowest.ts';

test('Playwright stops a test at 60 s and reports the ten slowest over 5 s', () => {
  assert.equal(config.timeout, 60_000);
  assert.deepEqual(config.reportSlowTests, { max: 10, threshold: 5_000 });
});

// reportSlowTests names only files that ran in one worker, so a parallel run prints nothing: the gates list tests themselves.
test('every Playwright run ends with its ten slowest tests, slowest first', () => {
  assert.deepEqual(config.reporter, [['line'], ['./ux/slowest.ts']]);
  const runs = Array.from({ length: 12 }, (_, i) => ({ title: `t${i}`, ms: i * 1000 }));
  const lines = slowest(runs);
  assert.equal(lines.length, 10);
  assert.equal(lines[0], '  11.0 s  t11');
  assert.equal(lines[9], '  2.0 s  t2');
  assert.deepEqual(slowest([{ title: 'only', ms: 450 }]), ['  0.5 s  only']);
});
