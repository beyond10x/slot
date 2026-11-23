import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { CacheMinute } from '../generated/types.ts';
import { FRESH_MS, MINUTE_MS, cachePoint, cacheWindow, describeMinute, emptyText, latestMinute, minuteAndDay, parseCacheDay, parseCacheHistory, percentOrDash } from './cache.ts';

const minute = (t: number, hits: number, misses: number, reasons: Record<string, number> = {}): CacheMinute => ({
  t, hits, misses, not_cached: Object.values(reasons).reduce((a, n) => a + n, 0), reasons,
});

test('a minute is two shares: of all calls, and of the calls the cache could have answered', () => {
  assert.deepEqual(cachePoint(minute(60_000, 26, 17, { 'crate-type': 50, incremental: 7 })), { t: 60_000, all: 26, cacheable: (26 / 43) * 100 });
  assert.deepEqual(cachePoint(minute(1, 0, 8)), { t: 1, all: 0, cacheable: 0 }, 'every call missed: 0 % is a value');
  assert.deepEqual(cachePoint(minute(1, 5, 0)), { t: 1, all: 100, cacheable: 100 });
});

test('a minute without a call has no share, and one with only refused calls none of the cacheable calls', () => {
  const none = cachePoint(minute(1, 0, 0));
  assert.ok(Number.isNaN(none.all) && Number.isNaN(none.cacheable));
  const refused = cachePoint(minute(1, 0, 0, { 'crate-type': 9 }));
  assert.equal(refused.all, 0);
  assert.ok(Number.isNaN(refused.cacheable));
});

test('the legend says a dash where there is no share, and 0 % only for a share of 0', () => {
  assert.equal(percentOrDash(undefined), '–');
  assert.equal(percentOrDash(NaN), '–');
  assert.equal(percentOrDash(0), '0 %');
  assert.equal(percentOrDash(26.4), '26 %');
  assert.equal(percentOrDash(100), '100 %');
  const refused = cachePoint(minute(1, 0, 0, { incremental: 3 }));
  assert.deepEqual([percentOrDash(refused.all), percentOrDash(refused.cacheable)], ['0 %', '–']);
});

test('the newest minute counts as now for three minutes, then the legend has no value', () => {
  const minutes = [minute(1_000_000, 1, 1), minute(1_060_000, 2, 2)];
  assert.equal(latestMinute(minutes, 1_060_000 + FRESH_MS)?.t, 1_060_000);
  assert.equal(latestMinute(minutes, 1_060_001 + FRESH_MS), undefined);
  assert.equal(latestMinute([], 5), undefined);
});

test('an answer of the daemon is read into minutes in the order of time, one per time', () => {
  assert.deepEqual(parseCacheHistory(undefined), []);
  assert.deepEqual(parseCacheHistory({ t: 1 }), []);
  assert.deepEqual(parseCacheHistory('<!doctype html>'), []);
  const got = parseCacheHistory([
    { t: 120_000, hits: 4, misses: 1, not_cached: 2, reasons: { incremental: 2, never: 0, broken: 'x' } },
    null, 7, { hits: 3 }, { t: 0, hits: 3 }, { t: 1.5, hits: 3 }, { t: '60000', hits: 3 },
    { t: 60_000, hits: 9, later_field: true },
    { t: 120_000, hits: 5, misses: -1, not_cached: 2, reasons: { incremental: 2 } },
  ]);
  assert.deepEqual(got, [
    { t: 60_000, hits: 9, misses: 0, not_cached: 0, reasons: {} },
    { t: 120_000, hits: 5, misses: 0, not_cached: 2, reasons: { incremental: 2 } },
  ]);
});

test('the window holds every minute that reaches into it', () => {
  const minutes = [100, 160, 220, 280].map((sec) => minute(sec * 1000, 1, 1));
  const inWindow = (from: number, to: number): number[] => cacheWindow(minutes, from * 1000, to * 1000).map((m) => m.t / 1000);
  assert.deepEqual(inWindow(100, 220), [160, 220], 'the minute that ends at 280 began at 220: it only touches the window');
  assert.deepEqual(inWindow(100, 221), [160, 220, 280]);
  assert.deepEqual(inWindow(99, 219), [100, 160, 220]);
  assert.deepEqual(inWindow(300, 900), []);
  assert.equal(MINUTE_MS, 60_000);
});

test('the chart says why it is empty: nothing recorded, or no call in the window', () => {
  assert.equal(emptyText(0, 0), 'no cache data yet');
  assert.equal(emptyText(120, 0), 'no compiler calls in this window');
  assert.equal(emptyText(120, 3), '');
});

test('the newest minute in words names the three most frequent reasons a call was not cacheable', () => {
  assert.equal(describeMinute(undefined), '');
  assert.equal(describeMinute(minute(1, 0, 0)), 'last minute: no compiler call');
  assert.equal(
    describeMinute(minute(1, 30, 12, { incremental: 4, 'crate-type': 40, '-': 4, 'multiple input files': 1 })),
    'last minute: 91 compiler calls, 30 from the cache, 12 compiled after a miss, 49 not cacheable (crate-type 40, - 4, incremental 4)',
  );
  assert.equal(describeMinute(minute(1, 2, 1)), 'last minute: 3 compiler calls, 2 from the cache, 1 compiled after a miss, 0 not cacheable');
});

test('the legend gives the newest minute and the last 24 hours, each a dash without calls or without an answer', () => {
  const day = cachePoint(minute(1, 350, 300, { incremental: 350 }));
  assert.equal(minuteAndDay(0, day.all), '0 % · 24 h: 35 %', 'a minute of misses beside the day the cache answered');
  assert.equal(minuteAndDay(68.4, day.cacheable), '68 % · 24 h: 54 %');
  assert.equal(minuteAndDay(undefined, day.all), '– · 24 h: 35 %', 'no fresh minute');
  assert.equal(minuteAndDay(26, undefined), '26 % · 24 h: –', 'a daemon without api/cache-day');
  const none = cachePoint(minute(1, 0, 0));
  assert.equal(minuteAndDay(none.all, none.all), '– · 24 h: –', 'no call in the day');
  const refused = cachePoint(minute(1, 0, 0, { incremental: 3 }));
  assert.deepEqual([minuteAndDay(1, refused.all), minuteAndDay(1, refused.cacheable)], ['1 % · 24 h: 0 %', '1 % · 24 h: –']);
});

test('an api/cache-day answer is one element; anything else is no figure', () => {
  assert.deepEqual(parseCacheDay({ t: 5, hits: 4, misses: 1, not_cached: 2, reasons: { incremental: 2 } }), minute(5, 4, 1, { incremental: 2 }));
  assert.deepEqual(parseCacheDay({ t: 5, hits: 4, later_field: true }), minute(5, 4, 0));
  for (const bad of [undefined, null, '<!doctype html>', [], [{ t: 5, hits: 4 }], { hits: 4 }, { t: 0 }]) {
    assert.equal(parseCacheDay(bad), undefined, JSON.stringify(bad));
  }
});
