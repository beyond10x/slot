import { test } from 'node:test';
import assert from 'node:assert/strict';
import { axisStepMs, axisTicks, fmtBytes, fmtDur } from './format.ts';

test('durations under 10 s carry one decimal', () => {
  assert.equal(fmtDur(4.24), '4.2s');
  assert.equal(fmtDur(0), '0.0s');
});

test('negative and NaN durations read as zero', () => {
  assert.equal(fmtDur(-3), '0.0s');
  assert.equal(fmtDur(Number.NaN), '0.0s');
});

test('durations from 10 s to 100 s are whole seconds', () => {
  assert.equal(fmtDur(42.4), '42s');
});

test('durations from 100 s are minutes and padded seconds', () => {
  assert.equal(fmtDur(187), '3m07');
  assert.equal(fmtDur(5999), '99m59');
});

test('durations from 6000 s are hours with one decimal', () => {
  assert.equal(fmtDur(5400 * 1.2), '1.8h');
});

test('zero bytes are empty', () => {
  assert.equal(fmtBytes(0), '');
});

test('bytes below 1 GiB are whole MB, above are GB', () => {
  assert.equal(fmtBytes(512 * 2 ** 20), '512 MB');
  assert.equal(fmtBytes(1.5 * 2 ** 30), '1.5 GB');
});

test('axis step keeps at most 10 ticks', () => {
  assert.equal(axisStepMs(120), 30_000);
  assert.equal(axisStepMs(600), 60_000);
  assert.equal(axisStepMs(21600), 3_600_000);
  assert.ok(72 * 3600 / (axisStepMs(72 * 3600) / 1000) <= 10);
});

test('axis ticks fall on step multiples inside the range', () => {
  assert.deepEqual(axisTicks(15_000, 75_000, 30_000), [30_000, 60_000]);
  assert.deepEqual(axisTicks(30_000, 60_000, 30_000), [30_000, 60_000]);
});
