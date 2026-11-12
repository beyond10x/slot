import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { CensusSession } from '../generated/types.ts';
import { familyColor, familyOrder, hostSample, notOnSlot, orderSessions, sessionLabel, stack } from './host.ts';
import { tick } from './fixtures.ts';

const session = (over: Partial<CensusSession>): CensusSession => ({
  session: '', title: '', cores: 0, unmanaged_cores: 0, rss: 0, procs: 0, ...over,
});

test('host sample splits each family into managed and unmanaged cores', () => {
  const t = tick(5, {
    tokens: 20,
    census: {
      host_cores: 14, build_cores: 9, unmanaged_cores: 3, psi_cpu: 12, psi_memory: 1, psi_io: 0.5, load1: 9,
      classes: [
        { class: 'rust-compile', managed: true, cores: 6, rss: 0, procs: 4 },
        { class: 'rust-compile', managed: false, cores: 2, rss: 0, procs: 1 },
        { class: 'node-test', managed: false, cores: 1, rss: 0, procs: 2 },
      ],
      sessions: [], recommended_tokens: 16, unmanaged_smoothed: 2.5,
    },
  });
  const s = hostSample(t);
  assert.deepEqual(s.byFamily, { 'rust-compile': { managed: 6, unmanaged: 2 }, 'node-test': { managed: 0, unmanaged: 1 } });
  assert.equal(s.recommended, 16);
  assert.equal(s.tokens, 20);
  assert.equal(s.psiCpu, 12);
});

test('family order: known families in fixed order, unknown after them by name, no duplicates', () => {
  assert.deepEqual(
    familyOrder(['zig-compile', 'link', 'node-test', 'rust-compile', 'go-test', 'awk', 'link']),
    ['rust-compile', 'go-test', 'node-test', 'link', 'awk', 'zig-compile'],
  );
});

test('stacking goes bottom up, managed before unmanaged, empty bands left out', () => {
  const bands = stack(
    { 'node-test': { managed: 0, unmanaged: 1 }, 'rust-compile': { managed: 6, unmanaged: 2 }, link: { managed: 0, unmanaged: 0 } },
    ['rust-compile', 'node-test', 'link'],
  );
  assert.deepEqual(bands, [
    { family: 'rust-compile', managed: true, y0: 0, y1: 6 },
    { family: 'rust-compile', managed: false, y0: 6, y1: 8 },
    { family: 'node-test', managed: false, y0: 8, y1: 9 },
  ]);
});

test('families missing from the order are not stacked', () => {
  assert.deepEqual(stack({ helm: { managed: 1, unmanaged: 0 } }, ['rust-compile']), []);
});

test('family colours: rust warm, go blue, node green, link and native grey, unknown stable', () => {
  assert.equal(familyColor('rust-compile'), '#D55E00');
  assert.equal(familyColor('go-compile'), '#0072B2');
  assert.equal(familyColor('node-test'), '#2E8540');
  assert.equal(familyColor('link'), '#6E6E6E');
  assert.equal(familyColor('zig'), familyColor('zig'));
  assert.match(familyColor('zig'), /^hsl\(/);
});

test('sessions sort by cores, then unmanaged cores, then label', () => {
  const got = orderSessions([
    session({ session: 'a', title: 'beta', cores: 2 }),
    session({ session: 'b', title: 'alpha', cores: 2 }),
    session({ session: 'c', title: 'gamma', cores: 2, unmanaged_cores: 1 }),
    session({ session: 'd', title: 'delta', cores: 9 }),
  ]);
  assert.deepEqual(got.map((s) => s.title), ['delta', 'gamma', 'alpha', 'beta']);
});

test('session label falls back to a short id, then a placeholder', () => {
  assert.equal(sessionLabel(session({ session: '0123456789abcdef', title: 'fix the parser' })), 'fix the parser');
  assert.equal(sessionLabel(session({ session: '0123456789abcdef' })), '01234567');
  assert.equal(sessionLabel(session({})), '(no session)');
});

test('a session is not on slot above one unmanaged core', () => {
  assert.equal(notOnSlot(session({ unmanaged_cores: 1 })), false);
  assert.equal(notOnSlot(session({ unmanaged_cores: 1.2 })), true);
});
