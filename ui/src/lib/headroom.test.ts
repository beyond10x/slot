import assert from 'node:assert/strict';
import { test } from 'node:test';
import { buildCores, effectiveTokens, headroomCaption } from './headroom.ts';

test('effective tokens match the daemon', () => {
  assert.equal(effectiveTokens(20, 0), 20);
  assert.equal(effectiveTokens(20, 25), 15);
  assert.equal(effectiveTokens(20, 30), 14);
  assert.equal(effectiveTokens(20, 50), 10);
  assert.equal(effectiveTokens(20, 90), 10);
  assert.equal(effectiveTokens(3, 50), 2);
  assert.equal(effectiveTokens(1, 50), 1);
});

test('caption names cores and slots', () => {
  assert.equal(headroomCaption(20, 20, 30), 'builds may use 14 of 20 cores, 14 of 20 slots');
  assert.equal(headroomCaption(20, 20, 0), 'builds may use 20 of 20 cores, 20 of 20 slots');
  assert.equal(buildCores(12, 25), 9);
  assert.equal(headroomCaption(6, 6, 25), 'builds may use 4.5 of 6 cores, 5 of 6 slots');
});
