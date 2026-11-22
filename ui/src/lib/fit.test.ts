import { test } from 'node:test';
import assert from 'node:assert/strict';
import { fitLabel } from './fit.ts';

/** Every character 7 px wide, as a canvas would measure a monospaced font. */
const width = (s: string): number => s.length * 7;

test('a label that fits is drawn whole', () => {
  assert.equal(fitLabel('parser: cargo test', 200, width), 'parser: cargo test');
  assert.equal(fitLabel('go', 20, width), 'go');
});

test('a label that does not fit ends with an ellipsis inside the room', () => {
  const got = fitLabel('parser: cargo clippy --workspace', 100, width);
  assert.equal(got, 'parser: cargo…');
  assert.ok(width(got) <= 100);
});

test('the cut does not leave a space before the ellipsis', () => {
  assert.equal(fitLabel('parser: cargo clippy', 63, width), 'parser:…');
});

test('no label when fewer than four characters fit beside the ellipsis', () => {
  assert.equal(fitLabel('parser: cargo clippy', 34, width), '');
  assert.equal(fitLabel('parser: cargo clippy', 35, width), 'pars…');
  assert.equal(fitLabel('parser: cargo clippy', 0, width), '');
});
