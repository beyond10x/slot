import { test } from 'node:test';
import assert from 'node:assert/strict';
import { SparkBuffer, sparkPath } from './spark.ts';
import { atBottom } from './scroll.ts';

test('buffer keeps only the last span per job', () => {
  const b = new SparkBuffer(60_000);
  for (let s = 0; s <= 100; s++) b.push(7, s * 1000, s);
  const pts = b.get(7);
  assert.equal(pts[0].t, 40_000);
  assert.equal(pts[pts.length - 1].t, 100_000);
  assert.equal(pts.length, 61);
});

test('buffer ignores samples that are not newer', () => {
  const b = new SparkBuffer();
  b.push(1, 2000, 1);
  b.push(1, 2000, 5);
  b.push(1, 1000, 5);
  assert.deepEqual(b.get(1), [{ t: 2000, v: 1 }]);
});

test('buffer forgets jobs that are no longer running', () => {
  const b = new SparkBuffer();
  b.push(1, 1000, 1);
  b.push(2, 1000, 2);
  b.retain(new Set([2]));
  assert.equal(b.size, 1);
  assert.deepEqual(b.get(1), []);
});

test('no points draw nothing', () => {
  assert.equal(sparkPath([], 0, 60_000, 60, 10, 1), '');
});

test('step path spans to the right edge and scales to vmax', () => {
  const d = sparkPath([{ t: 30_000, v: 2 }, { t: 45_000, v: 4 }], 60_000, 60_000, 60, 10, 4);
  assert.equal(d, 'M30,5H45V0H60');
});

test('points before the span clamp to the left edge, values above vmax to the top', () => {
  assert.equal(sparkPath([{ t: -5000, v: 9 }], 60_000, 60_000, 60, 10, 4), 'M0,0H60');
});

test('at bottom within slack', () => {
  assert.equal(atBottom(780, 200, 1000), true);
  assert.equal(atBottom(700, 200, 1000), false);
  assert.equal(atBottom(0, 200, 150), true);
});
