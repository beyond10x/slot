import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { Job, Node } from '../generated/types.ts';
import { checkoutOf, cloudCost, cpuTotals, fmtLeft, jobList, lifeLeft, localFromState, nodeJobs, nodeOf, parseNodes, slotCells, specLine } from './cluster.ts';

const T = Date.UTC(2026, 10, 12, 9, 0, 0);
const iso = (ms: number): string => new Date(ms).toISOString();

function job(id: number, over: Partial<Job> = {}): Job {
  return {
    id, argv: ['cargo', 'build'], cwd: '/w/ledger', repo: 'ledger', session: '', client: '', toolchain: 'rust', remote: false, tokens: 2,
    state: 'running', enqueued_ms: T - 10_000, started_ms: T - 5_000, ended_ms: 0, exit: 0, cpu_s: 0, compile_server_cpu_s: 0, peak_rss: 0,
    wait_reason: '', target_dir: '', priority: 'normal', promoted: false, node: 'local', ...over,
  } as Job;
}

function row(over: Partial<Node> = {}): Node {
  return {
    node_id: 'local', cluster_id: 'h', kind: 'Local', state: 'Ready', cpus: 20, slots: 16, slots_in_use: 0, memory_gib: 32, disk_gib: 1000,
    running_jobs: 0, queued_jobs: 0, slot_version: '0.30.0', ...over,
  };
}

test('api/nodes is a bare array; rows without a node id are dropped; anything else is no answer', () => {
  assert.deepEqual(parseNodes([row(), { node_id: '' }]), [row()]);
  assert.equal(parseNodes({ nodes: [] }), null);
});

test('a job without a node ran on the local node; checkouts name a linked worktree', () => {
  assert.equal(nodeOf(job(1, { node: '' })), 'local');
  assert.equal(checkoutOf(job(1, { checkout: { kind: 'worktree', name: 'fix' } })), 'ledger/fix');
  assert.equal(checkoutOf(job(1, { repo: '', cwd: '/jobs/build-cache' })), 'build-cache');
});

test('a node lists the checkouts of its active jobs once and counts its finished and failed jobs', () => {
  const jobs = [
    job(1, { checkout: { kind: 'worktree', name: 'a' } }), job(2, { state: 'queued', checkout: { kind: 'worktree', name: 'a' } }),
    job(3, { state: 'ok' }), job(4, { state: 'failed' }), job(5, { node: 'cloud-a' }),
  ];
  assert.deepEqual(nodeJobs('local', jobs), { checkouts: ['ledger/a'], done: 1, failed: 1 });
  assert.deepEqual(nodeJobs('cloud-a', jobs), { checkouts: ['ledger'], done: 0, failed: 0 });
});

test('an older daemon gives the local node from the state: slots from the limit, CPU from the jobs', () => {
  const n = localFromState({ host: 'h', cores: 20, limit: 16, tokens: 20, free: 10, jobCores: 3.25, diskFree: 0 }, [job(1), job(2, { state: 'queued' })]);
  assert.equal(n.slots, 16);
  assert.equal(n.slots_in_use, 6);
  assert.equal(n.cpu_busy_milli, 3250);
  assert.deepEqual([n.running_jobs, n.queued_jobs, n.memory_gib], [1, 1, 0]);
  assert.equal(n.disk_free_gib, undefined);
});

test('CPUs count over ready and draining nodes; cloud cost runs from the join to now or the deadline', () => {
  const cloud = row({
    node_id: 'c', kind: 'Remote', state: 'Draining', cpus: 64, cpu_busy_milli: 32_000, price_per_hour_cents: 200,
    joined_at: iso(T - 3_600_000), deadline: iso(T + 1_800_000),
  });
  assert.deepEqual(cpuTotals([row({ cpu_busy_milli: 1500 }), cloud, row({ state: 'Gone', cpus: 8 })]), { used: 33.5, available: 84 });
  assert.equal(cloudCost([row()], T), null);
  assert.deepEqual(cloudCost([cloud], T), { total: 2, perHour: 2 });
  assert.deepEqual(cloudCost([cloud], T + 7_200_000), { total: 3, perHour: 2 });
  const l = lifeLeft(cloud, T);
  assert.ok(l && Math.abs(l.frac - 1 / 3) < 1e-9 && l.leftS === 1800 && l.spanH === 1.5);
  assert.equal(lifeLeft(row(), T), null);
  assert.equal(fmtLeft(7_980), '2:13');
  assert.equal(fmtLeft(35), '0:35');
  assert.equal(specLine(cloud), '64 vCPU · 16 slots · 32 GiB · 1000 GiB disk · 2.00/h');
});

test('slot cells fill in start order by the slots each running job holds', () => {
  const cells = slotCells(5, [job(2, { started_ms: T, tokens: 2, toolchain: 'go' }), job(1, { started_ms: T - 1, tokens: 2 })]);
  assert.deepEqual(cells.map((c) => c?.id ?? null), [1, 1, 2, 2, null]);
});

test('the job list is newest first and keeps one node under a filter', () => {
  const jobs = [job(1), job(3, { node: 'c' }), job(2)];
  assert.deepEqual(jobList(jobs, null, 10).map((j) => j.id), [3, 2, 1]);
  assert.deepEqual(jobList(jobs, 'local', 1).map((j) => j.id), [2]);
});
