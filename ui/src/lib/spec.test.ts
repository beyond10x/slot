import { test } from 'node:test';
import assert from 'node:assert/strict';
import { among, contains, eq, href, parseHash, viewUrl } from './spec.ts';
import { formatValue, toneOf } from './spec-format.ts';

test('contains keeps a row that holds every word in any case, and every row for an empty search', () => {
  assert.equal(contains(['cargo', 'test', '--locked'], 'TEST cargo'), true);
  assert.equal(contains(['cargo', 'test'], 'clippy'), false);
  assert.equal(contains('running', ''), true);
  assert.equal(contains(undefined, 'x'), false);
});

test('== and in compare values as text', () => {
  assert.equal(eq(61259, '61259'), true);
  assert.equal(among('failed', ['ok', 'failed']), true);
  assert.equal(among('queued', ['ok', 'failed']), false);
});

test('a hash names a generated page with its params and its page state; no hash is the dashboard', () => {
  assert.equal(parseHash(''), null);
  assert.equal(parseHash('#/'), null);
  assert.equal(parseHash('#/nowhere'), null);
  const list = parseHash('#/jobs?search=cargo&state=ok');
  assert.equal(list?.page, 'jobs.list');
  assert.equal(list?.query.get('search'), 'cargo');
  assert.deepEqual(parseHash('#/jobs/42')?.params, { id: '42' });
  assert.equal(href('jobs.detail', { id: '42' }), '#/jobs/42');
});

test('a view read fills its path variables from its params', () => {
  assert.equal(viewUrl('slot.pool.JobDetail', { id: '42' }), 'api/jobs/42');
  assert.equal(viewUrl('slot.pool.Jobs', {}), 'api/jobs');
});

test('values are drawn by the as of their field', () => {
  assert.equal(formatValue(61259, {}), '61259');
  assert.equal(formatValue(['cargo', 'test'], { as: 'list' }), 'cargo test');
  assert.equal(formatValue(1 << 30, { as: 'bytes' }), '1.0 GB');
  assert.equal(formatValue(90, { as: 'duration', unit: 's' }), '90s');
  assert.equal(formatValue(0, { as: 'time' }), '');
  assert.equal(formatValue(2.25, { as: 'number' }), '2.3');
  assert.equal(formatValue({ A: '1' }, { as: 'map' }), 'A=1');
  assert.equal(toneOf('failed', { tones: { failed: 'danger' } }), 'danger');
  assert.equal(toneOf('odd', { tones: { failed: 'danger' } }), 'neutral');
});
