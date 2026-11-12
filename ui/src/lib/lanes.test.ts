import { test } from 'node:test';
import assert from 'node:assert/strict';
import { assignLanes } from './lanes.ts';

type S = { id: string; s: number; e: number };
const run = (items: S[]) => {
  const r = assignLanes(items, (x) => x.s, (x) => x.e);
  return { lanes: r.lanes, at: Object.fromEntries(r.placed.map((p) => [p.item.id, p.lane])) };
};

test('no items need no lanes', () => {
  assert.deepEqual(run([]), { lanes: 0, at: {} });
});

test('overlapping items take separate lanes', () => {
  assert.deepEqual(run([{ id: 'a', s: 0, e: 10 }, { id: 'b', s: 5, e: 15 }]), { lanes: 2, at: { a: 0, b: 1 } });
});

test('an item starting when another ends reuses its lane', () => {
  assert.deepEqual(run([{ id: 'a', s: 0, e: 10 }, { id: 'b', s: 10, e: 20 }]), { lanes: 1, at: { a: 0, b: 0 } });
});

test('open items (end Infinity) keep their lane', () => {
  const r = run([
    { id: 'a', s: 0, e: Infinity },
    { id: 'b', s: 1, e: 2 },
    { id: 'c', s: 3, e: 4 },
  ]);
  assert.deepEqual(r, { lanes: 2, at: { a: 0, b: 1, c: 1 } });
});

test('the lowest free lane is taken', () => {
  const r = run([
    { id: 'a', s: 0, e: 5 },
    { id: 'b', s: 1, e: 9 },
    { id: 'c', s: 2, e: 9 },
    { id: 'd', s: 6, e: 8 },
  ]);
  assert.deepEqual(r.at, { a: 0, b: 1, c: 2, d: 0 });
});

test('input order does not matter', () => {
  const items = [{ id: 'b', s: 5, e: 15 }, { id: 'a', s: 0, e: 10 }];
  assert.deepEqual(run(items).at, { a: 0, b: 1 });
});
