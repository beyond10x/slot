import assert from 'node:assert/strict';
import { test } from 'node:test';
import { atBottom, revealDelta } from './scroll.ts';

test('at the bottom within the slack', () => {
  assert.equal(atBottom(780, 200, 1000), true);
  assert.equal(atBottom(700, 200, 1000), false);
});

test('a box in the band needs no scroll', () => {
  assert.equal(revealDelta(300, 330, 160, 600), 0);
});

test('a box under the bottom sheet scrolls up just above it', () => {
  // Row at 700..730, sheet starts at 594: scroll 730 + 8 - 594.
  assert.equal(revealDelta(700, 730, 160, 594), 144);
});

test('a box under the sticky header scrolls down below it', () => {
  assert.equal(revealDelta(100, 130, 160, 594), -68);
});

test('a box taller than the band is aligned at its top', () => {
  assert.equal(revealDelta(400, 1000, 160, 594), 232);
});
