import { test } from 'node:test';
import assert from 'node:assert/strict';
import { hashHue, stackOrder, tcColor } from './colors.ts';

test('known toolchains keep their fixed colours', () => {
  assert.equal(tcColor('rust'), '#D55E00');
  assert.equal(tcColor('go'), '#0072B2');
  assert.equal(tcColor('other'), '#8c8c8c');
});

test('unknown toolchains get a stable hashed hsl colour', () => {
  assert.equal(tcColor('zig'), `hsl(${hashHue('zig')},60%,42%)`);
  assert.equal(tcColor('zig'), tcColor('zig'));
  assert.notEqual(hashHue('zig'), hashHue('node'));
});

test('hash hue stays in 0..359 for long names', () => {
  const h = hashHue('x'.repeat(10_000));
  assert.ok(Number.isInteger(h) && h >= 0 && h < 360);
});

test('stack order puts fixed toolchains first, others in first-seen order', () => {
  assert.deepEqual(stackOrder(['zig', 'go', 'node', 'zig']), ['rust', 'go', 'other', 'zig', 'node']);
});
