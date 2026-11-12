import assert from 'node:assert/strict';
import { test } from 'node:test';
import { compare, hardFailures, hostDependent, toleranceFor } from './ratchet-lib.ts';
import type { Baseline } from './ratchet-lib.ts';

const base = (value: number, decision?: string): Baseline => ({
  about: '', tolerances: {},
  scenarios: { s: { 'screen_only.actions': { value, tolerance: 0, ...(decision ? { decision } : {}) } } },
});

test('equal values pass', () => {
  assert.deepEqual(compare(base(2), { s: { 'screen_only.actions': 2 } }).failures, []);
});

test('a worse value fails', () => {
  const o = compare(base(2), { s: { 'screen_only.actions': 3 } });
  assert.equal(hardFailures(o).length, 1);
});

test('an improvement must be taken into the baseline', () => {
  const o = compare(base(2), { s: { 'screen_only.actions': 1 } });
  assert.match(o.failures[0], /take it in/);
  assert.equal(hardFailures(o).length, 0);
  assert.equal(o.next.scenarios.s['screen_only.actions'].value, 1);
});

test('a missing value fails', () => {
  assert.match(compare(base(2), { s: {} }).failures[0], /missing/);
});

test('a baseline worse than main needs a decision id', () => {
  assert.match(compare(base(3), { s: { 'screen_only.actions': 3 } }, base(2)).failures[0], /names no decision/);
  assert.deepEqual(compare(base(3, 'R-01-D1'), { s: { 'screen_only.actions': 3 } }, base(2)).failures, []);
});

test('reading load tolerates 10 %, counts nothing', () => {
  assert.equal(toleranceFor('reading_load.max', 205), 21);
  assert.equal(toleranceFor('screen_only.actions', 2), 0);
  assert.equal(toleranceFor('layout_shift.per_minute', 0.5), 0.01);
});

test('keys that depend on the host fonts are carried over on a merge', () => {
  const b: Baseline = { about: '', tolerances: {}, scenarios: { s: { 'reading_load.max': { value: 100, tolerance: 10 } } } };
  assert.deepEqual(compare(b, { s: { 'reading_load.max': 300 } }, undefined, hostDependent).failures, []);
  assert.equal(compare(b, { s: { 'reading_load.max': 300 } }).failures.length, 1);
});
