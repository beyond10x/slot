import assert from 'node:assert/strict';
import { test } from 'node:test';
import { POOL_BASE, buildCores, effectiveTokens, headroomCaption, machineLine, machineNumbers, notJsonError } from './headroom.ts';

test('effective tokens match the daemon', () => {
  assert.equal(effectiveTokens(20, 0), 20);
  assert.equal(effectiveTokens(20, 25), 15);
  assert.equal(effectiveTokens(20, 30), 14);
  assert.equal(effectiveTokens(20, 50), 10);
  assert.equal(effectiveTokens(20, 90), 10);
  assert.equal(effectiveTokens(3, 50), 2);
  assert.equal(effectiveTokens(1, 50), 1);
});

test('caption names cores and slots', () => {
  assert.equal(headroomCaption(20, 20, 30), 'builds may use 14 of 20 cores, 14 of 20 slots');
  assert.equal(headroomCaption(20, 20, 0), 'builds may use 20 of 20 cores, 20 of 20 slots');
  assert.equal(buildCores(12, 25), 9);
  assert.equal(headroomCaption(6, 6, 25), 'builds may use 4.5 of 6 cores, 5 of 6 slots');
});

test('the machine line names the load outside the pool, as slot status does', () => {
  const m = {
    headroom_base: 'machine' as const, busy_cores: 18.24, target_cores: 11, outside_cores: 9.1, quota_floor: true, over_target: true,
    outside: [{ name: 'session-3.scope', cores: 5.9 }, { name: 'docker 3f2a1b2c3d4e', cores: 3.2 }],
  };
  assert.equal(
    machineLine(m, 20, 200),
    'machine (over target): 18.2 of 20 CPUs busy, target 11.0, pool quota 2.0 (floor), outside 9.1: session-3.scope 5.9, docker 3f2a1b2c3d4e 3.2',
  );
  assert.equal(
    machineLine({ ...m, quota_floor: false, over_target: false, outside: [] }, 20, 430),
    'machine: 18.2 of 20 CPUs busy, target 11.0, pool quota 4.3, outside 9.1',
  );
  assert.match(machineLine(m, 20, null) ?? '', /pool quota not applied,/);
  assert.equal(machineLine({ ...m, headroom_base: 'pool' }, 20, 200), null);
  assert.equal(machineLine({ ...m, target_cores: 0 }, 20, null), null, 'no headroom: no line');
});

test('a state from an older daemon reads as the pool base', () => {
  assert.deepEqual(machineNumbers({}), POOL_BASE);
  assert.equal(machineNumbers({ headroom_base: 'machine', target_cores: 11 }).target_cores, 11);
});

test('the caption on the machine base speaks of the machine', () => {
  assert.equal(headroomCaption(20, 20, 45, 'machine'), 'the machine may use 11 of 20 cores, builds 11 of 20 slots');
  assert.equal(headroomCaption(20, 20, 30, 'pool'), 'builds may use 14 of 20 cores, 14 of 20 slots');
});

test('an answer that is not JSON comes from an older daemon', () => {
  assert.equal(notJsonError('application/json'), null);
  assert.match(notJsonError('text/html; charset=utf-8') ?? '', /older than this page/);
  assert.match(notJsonError(null) ?? '', /older than this page/);
});
