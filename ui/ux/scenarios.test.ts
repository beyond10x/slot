// The scenario data answers the API as the daemon would.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { SCENARIOS } from './scenarios.ts';

test('usage rows come in the daemon\'s order: slot time descending, then key', () => {
  const withRows = SCENARIOS.filter((s) => s.usage && s.usage.rows.length > 1);
  assert.ok(withRows.length > 0, 'a scenario answers api/usage with rows');
  for (const sc of withRows) {
    const rows = sc.usage?.rows ?? [];
    const sorted = [...rows].sort((a, b) => b.slot_s - a.slot_s || (a.key < b.key ? -1 : a.key > b.key ? 1 : 0));
    assert.deepEqual(rows.map((r) => r.key), sorted.map((r) => r.key), sc.name);
  }
});
