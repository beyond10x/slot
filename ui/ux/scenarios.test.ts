// The scenario data answers the API as the daemon would.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { SCENARIOS, T0 } from './scenarios.ts';

test('usage rows come in the daemon\'s order: slot time descending, then key', () => {
  const withRows = SCENARIOS.filter((s) => s.usage && s.usage.rows.length > 1);
  assert.ok(withRows.length > 0, 'a scenario answers api/usage with rows');
  for (const sc of withRows) {
    const rows = sc.usage?.rows ?? [];
    const sorted = [...rows].sort((a, b) => b.slot_s - a.slot_s || (a.key < b.key ? -1 : a.key > b.key ? 1 : 0));
    assert.deepEqual(rows.map((r) => r.key), sorted.map((r) => r.key), sc.name);
  }
});

test('api/cache-history is answered with calls, with an empty array and with 404, and the calls are as the daemon sends them', () => {
  const withCalls = SCENARIOS.filter((s) => s.cacheHistory && s.cacheHistory.length > 0);
  assert.ok(withCalls.length > 0, 'a scenario with compiler calls');
  assert.ok(SCENARIOS.some((s) => s.cacheHistory?.length === 0), 'a scenario with an empty array');
  assert.ok(SCENARIOS.some((s) => s.cacheHistory === undefined), 'a scenario whose daemon has no such path');
  for (const sc of withCalls) {
    const minutes = sc.cacheHistory ?? [];
    assert.deepEqual(minutes.map((m) => m.t), [...minutes.map((m) => m.t)].sort((a, b) => a - b), `${sc.name}: in the order of time`);
    assert.equal(new Set(minutes.map((m) => m.t)).size, minutes.length, `${sc.name}: one element per time`);
    for (const m of minutes) {
      assert.equal(m.not_cached, Object.values(m.reasons).reduce((a, n) => a + n, 0), `${sc.name}: not_cached is the sum of its reasons`);
      assert.ok(m.t <= T0, `${sc.name}: no reading after the scenario's start`);
    }
  }
});
