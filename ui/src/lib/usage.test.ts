import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { Usage, UsageRow } from '../generated/types.ts';
import { usageRows, usageUrl, windowStart } from './usage.ts';

const row = (key: string, slot_share: number, extra: Partial<UsageRow> = {}): UsageRow => ({
  key,
  title: '',
  jobs: 10,
  slot_s: slot_share * 36000,
  cpu_s: slot_share * 18000,
  wait_s: 100,
  wait_p50_s: 42,
  wait_max_s: 300,
  slot_share,
  cpu_share: slot_share / 2,
  ...extra,
});

const usage = (rows: UsageRow[]): Usage => ({
  from_ms: 0,
  to_ms: 1,
  by: 'project',
  total: row('', 1),
  rows,
});

test('today starts at local midnight; 24 h and 7 d count back from now', () => {
  const now = new Date(2026, 10, 13, 15, 30, 0);
  assert.equal(windowStart('today', now), new Date(2026, 10, 13, 0, 0, 0).getTime());
  assert.equal(windowStart('24h', now), now.getTime() - 24 * 3600_000);
  assert.equal(windowStart('7d', now), now.getTime() - 7 * 86_400_000);
});

test('the request names the window and the grouping', () => {
  const now = new Date(2026, 10, 13, 15, 30, 0);
  const url = usageUrl('24h', 'session', now);
  assert.equal(url, `api/usage?from=${now.getTime() - 24 * 3600_000}&to=${now.getTime()}&by=session`);
});

test('rows show shares, hours and the median wait, biggest first, at most top', () => {
  const rows = usageRows(usage([row('team-a', 0.366), row('team-b', 0.092), row('', 0.01)]), 2);
  assert.equal(rows.length, 2);
  assert.deepEqual(
    { label: rows[0].label, share: rows[0].share, cpu: rows[0].cpuShare, hours: rows[0].slotHours, wait: rows[0].wait },
    { label: 'team-a', share: '36.6%', cpu: '18.3%', hours: '3.7', wait: '42s' },
  );
  assert.equal(rows[0].width, 100, 'the largest row fills the bar track');
  assert.ok(Math.abs(rows[1].width - (0.092 / 0.366) * 100) < 1e-9);
  assert.equal(usageRows(usage([row('', 1)]), 5)[0].label, '(none)');
});

test('a session row is labelled by its title, else by the start of its id', () => {
  const u = usage([row('0b1f9e2a-6c1d', 0.5, { title: 'checkout service' }), row('7c2e1d4f-8a9b', 0.5)]);
  u.by = 'session';
  const rows = usageRows(u, 5);
  assert.equal(rows[0].label, 'checkout service');
  assert.equal(rows[1].label, '7c2e1d4f');
});

test('an empty window has no rows', () => {
  assert.deepEqual(usageRows(usage([]), 5), []);
});
