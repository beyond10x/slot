import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { LiveJob, Unit } from '../generated/types.ts';
import { kindStyle, orderUnits, phase, unitText, unitsDone } from './units.ts';

const u = (kind: string, name = kind, cores = 1, secs = 1): Unit => ({ kind, name, cores, secs });

test('no units means running', () => {
  assert.equal(phase([]), 'running');
});

test('any compile, build-script or native unit means compiling', () => {
  assert.equal(phase([u('link'), u('compile')]), 'compiling');
  assert.equal(phase([u('test'), u('build-script')]), 'compiling');
  assert.equal(phase([u('native')]), 'compiling');
});

test('tests without compiling mean testing, also next to a link', () => {
  assert.equal(phase([u('test')]), 'testing');
  assert.equal(phase([u('link'), u('test')]), 'testing');
});

test('only link units mean linking', () => {
  assert.equal(phase([u('link', 'a'), u('link', 'b')]), 'linking');
});

test('unknown kinds alone mean running', () => {
  assert.equal(phase([u('go-vet')]), 'running');
  assert.equal(phase([u('link'), u('go-vet')]), 'running');
});

test('chips order by cores, then seconds, then name', () => {
  const got = orderUnits([u('compile', 'b', 1, 5), u('compile', 'a', 1, 5), u('link', 'z', 2, 1), u('test', 'c', 1, 9)]);
  assert.deepEqual(got.map((x) => x.name), ['z', 'c', 'a', 'b']);
});

test('ordering does not change the input', () => {
  const list = [u('compile', 'a', 1), u('compile', 'b', 3)];
  orderUnits(list);
  assert.deepEqual(list.map((x) => x.name), ['a', 'b']);
});

test('units done is seen minus running, never negative', () => {
  const l: LiveJob = { id: 1, cores: 2, rss: 0, procs: 3, units: [u('compile'), u('link')], units_seen: 40 };
  assert.equal(unitsDone(l), 38);
  assert.equal(unitsDone({ ...l, units_seen: 1 }), 0);
});

test('known kinds have their own label and colour, unknown keep their name', () => {
  assert.deepEqual(kindStyle('build-script'), { label: 'build.rs', color: '#E69F00' });
  assert.notEqual(kindStyle('compile').color, kindStyle('test').color);
  assert.deepEqual(kindStyle('wasm-opt'), { label: 'wasm-opt', color: '#8c8c8c' });
});

test('chip text shows name, cores and time', () => {
  assert.equal(unitText(u('compile', 'serde', 1.84, 12), (s) => `${s}s`), 'serde · 1.8 cores · 12s');
});
