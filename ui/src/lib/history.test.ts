import { test } from 'node:test';
import assert from 'node:assert/strict';
import { tick } from './fixtures.ts';
import { coresSample, mergeByT, parseHistory } from './history.ts';
import { SparkBuffer } from './spark.ts';

const ts = (xs: ReadonlyArray<{ t: number }>) => xs.map((x) => x.t);

test('merge keeps order and one entry per t', () => {
  const got = mergeByT([{ t: 1 }, { t: 2 }, { t: 3 }], [{ t: 3 }, { t: 4 }]);
  assert.deepEqual(ts(got), [1, 2, 3, 4]);
});

test('on a tie the newer list wins', () => {
  const got = mergeByT([{ t: 2, v: 'history' }], [{ t: 2, v: 'live' }]);
  assert.deepEqual(got, [{ t: 2, v: 'live' }]);
});

test('history fully overlapped by live samples adds nothing', () => {
  const live = [{ t: 5 }, { t: 6 }];
  assert.deepEqual(ts(mergeByT([{ t: 5 }, { t: 6 }], live)), [5, 6]);
});

test('interleaved lists merge, and duplicates inside one list collapse', () => {
  assert.deepEqual(ts(mergeByT([{ t: 1 }, { t: 3 }, { t: 3 }], [{ t: 2 }, { t: 4 }])), [1, 2, 3, 4]);
});

test('merge caps to the newest entries', () => {
  assert.deepEqual(ts(mergeByT([{ t: 1 }, { t: 2 }], [{ t: 3 }], 2)), [2, 3]);
});

test('empty inputs', () => {
  assert.deepEqual(mergeByT([], []), []);
  assert.deepEqual(ts(mergeByT([], [{ t: 1 }])), [1]);
});

test('history parsing keeps well-formed ticks, sorted, one per t', () => {
  const got = parseHistory([tick(3), { type: 'job' }, tick(1), null, tick(3), { type: 'tick', t: 'x' }, { type: 'tick', t: 9 }]);
  assert.deepEqual(ts(got), [1, 3]);
  assert.deepEqual(parseHistory({ not: 'an array' }), []);
});

test('cores sample sums job cores per toolchain, unknown jobs count as other', () => {
  const s = coresSample(
    tick(7, {
      waiting: 2, sys_cpu: 0.5,
      jobs: [
        { id: 1, cores: 2, rss: 0, procs: 1, units: [], units_seen: 0 },
        { id: 2, cores: 1.5, rss: 0, procs: 1, units: [], units_seen: 0 },
        { id: 3, cores: 1, rss: 0, procs: 1, units: [], units_seen: 0 },
      ],
    }),
    (id) => (id === 3 ? undefined : 'rust'),
    20,
  );
  assert.deepEqual(s, { t: 7, sys: 10, by: { rust: 3.5, other: 1 }, waiting: 2 });
});

test('seeding a spark buffer under live points adds no duplicates and keeps the span', () => {
  const b = new SparkBuffer<string>(60_000);
  b.push('s', 100_000, 5);
  b.push('s', 101_000, 6);
  const history = Array.from({ length: 120 }, (_, k) => ({ t: (k - 18) * 1000, v: 1 }));
  b.seed('s', history);
  const pts = b.get('s');
  assert.equal(pts[0].t, 41_000);
  assert.equal(pts[pts.length - 1].t, 101_000);
  assert.equal(new Set(pts.map((p) => p.t)).size, pts.length);
  assert.deepEqual(pts.slice(-2), [{ t: 100_000, v: 5 }, { t: 101_000, v: 6 }]);
});

test('seeding an unknown key starts it', () => {
  const b = new SparkBuffer<number>();
  b.seed(4, [{ t: 1000, v: 1 }]);
  assert.deepEqual(b.get(4), [{ t: 1000, v: 1 }]);
  b.seed(5, []);
  assert.equal(b.size, 1);
});
