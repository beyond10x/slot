import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { Job, Node } from '../generated/types.ts';
import {
  checkoutOf, cloudCost, cpuTotals, failText, failedHour, fmtLeft, hostLine, jobList, leaseMismatch, lifeLeft, localFromState, nodeCounts, nodeJobs, nodeOf,
  outcomeOf, parseNodes, slotCells, specLine,
} from './cluster.ts';

const T = Date.UTC(2026, 10, 12, 9, 0, 0);
const iso = (ms: number): string => new Date(ms).toISOString();

function job(id: number, over: Partial<Job> = {}): Job {
  return {
    id, argv: ['cargo', 'build'], cwd: '/w/ledger', repo: 'ledger', session: '', client: '', toolchain: 'rust', remote: false, tokens: 2,
    state: 'running', enqueued_ms: T - 10_000, started_ms: T - 5_000, ended_ms: 0, exit: 0, cpu_s: 0, compile_server_cpu_s: 0, peak_rss: 0,
    wait_reason: '', local_reason: '', target_dir: '', priority: 'normal', promoted: false, node: 'local', ...over,
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

test('a node lists the checkouts of its active jobs once', () => {
  const jobs = [
    job(1, { checkout: { kind: 'worktree', name: 'a' } }), job(2, { state: 'queued', checkout: { kind: 'worktree', name: 'a' } }),
    job(3, { state: 'ok' }), job(4, { state: 'failed' }), job(5, { node: 'cloud-a' }),
  ];
  assert.deepEqual(nodeJobs('local', jobs), { checkouts: ['ledger/a'] });
  assert.deepEqual(nodeJobs('cloud-a', jobs), { checkouts: ['ledger'] });
});

test('an older daemon gives the local node from the state: slots from the limit, CPU from the jobs', () => {
  const n = localFromState({ host: 'h', cores: 20, limit: 16, tokens: 20, free: 10, jobCores: 3.25, diskFree: 0 }, [job(1), job(2, { state: 'queued' })]);
  assert.equal(n.slots, 16);
  assert.equal(n.slots_in_use, 6);
  assert.equal(n.cpu_busy_milli, 3250);
  assert.deepEqual([n.running_jobs, n.queued_jobs, n.memory_gib], [1, 1, 0]);
  assert.equal(n.disk_free_gib, undefined);
});

test('CPUs count over ready and draining nodes; cloud cost runs from the join to now, past the deadline', () => {
  const cloud = row({
    node_id: 'c', kind: 'Remote', state: 'Draining', cpus: 64, cpu_busy_milli: 32_000, price_per_hour_cents: 200,
    joined_at: iso(T - 3_600_000), deadline: iso(T + 1_800_000),
  });
  assert.deepEqual(cpuTotals([row({ cpu_busy_milli: 1500 }), cloud, row({ state: 'Gone', cpus: 8 })]), { used: 33.5, available: 84 });
  assert.equal(cloudCost([row()], T), null);
  assert.deepEqual(cloudCost([cloud], T), { total: 2, perHour: 2 });
  assert.deepEqual(cloudCost([cloud], T + 7_200_000), { total: 6, perHour: 2 });
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

test('a job of a remote node, with an id below 0, takes its place in the list by when it was queued', () => {
  const jobs = [job(1, { enqueued_ms: T - 3 }), job(-1000000000007, { node: 'build-1', enqueued_ms: T - 1 }), job(2, { enqueued_ms: T - 2 })];
  assert.deepEqual(jobList(jobs, null, 2).map((j) => j.id), [-1000000000007, 2]);
});

const H = 3_600_000;

test('UXF-039: the lease ring counts to the poweroff the node reports, marked when the cluster file says otherwise', () => {
  const stale = row({
    node_id: 'build-1d', kind: 'Remote', joined_at: iso(T - 2 * H), deadline: iso(T - 4 * H), booted_at: iso(T - 12 * H), poweroff_at: iso(T + 2 * H),
  });
  const l = lifeLeft(stale, T);
  assert.ok(l, 'a lease');
  assert.equal(l.leftS, 7200, 'the node is running: two hours left, not 0:00');
  assert.equal(l.spanH, 14, 'from the boot to the poweroff');
  const m = leaseMismatch(stale);
  assert.match(m?.text ?? '', /the node powers off at .*, the cluster file says /);
  assert.match(m?.short ?? '', /^file /, 'under the ring: what the file says');
  const agree = row({ ...stale, deadline: iso(T + 2 * H + 4 * 60_000) });
  assert.equal(leaseMismatch(agree), null, 'within 5 minutes they agree');
  const none = row({ ...stale, poweroff_at: undefined });
  assert.match(leaseMismatch(none)?.text ?? '', /no poweroff scheduled/);
  const older = row({ ...stale, booted_at: undefined, poweroff_at: undefined });
  assert.equal(leaseMismatch(older), null, 'a node daemon that reports neither: nothing to compare');
  assert.equal(lifeLeft(older, T)?.leftS, 0, 'then the ring follows the cluster file');
  assert.equal(leaseMismatch(row({ booted_at: iso(T) })), null, 'the local node has no lease');
});

test('UXF-040: cloud cost runs from the node\'s boot to now, still counting past the deadline', () => {
  const c = row({ node_id: 'build-1d', kind: 'Remote', price_per_hour_cents: 373, joined_at: iso(T - 5.5 * H), deadline: iso(T - 4 * H), booted_at: iso(T - 12 * H) });
  const cost = cloudCost([c], T);
  assert.ok(cost && Math.abs(cost.total - 3.73 * 12) < 1e-9, `${cost?.total}`);
  assert.ok(Math.abs((cloudCost([c], T + H)?.total ?? 0) - 3.73 * 13) < 1e-9, 'an hour later, an hour more');
  const gone = row({ ...c, state: 'Gone', left_at: iso(T - 2 * H) });
  assert.ok(Math.abs((cloudCost([gone], T)?.total ?? 0) - 3.73 * 10) < 1e-9, 'a node that left: to its leaving');
});

const out = (o: Partial<Record<'ok' | 'test' | 'timeout' | 'infra' | 'cancelled', number>>) => ({ ok: 0, test: 0, timeout: 0, infra: 0, cancelled: 0, ...o });

test('UXF-041: every node counts its own finished jobs over the last 60 min and today', () => {
  const midnight = T - 9 * H;
  const node = row({
    node_id: 'build-1d', kind: 'Remote', ended_hour: out({ ok: 120, test: 3, infra: 1 }),
    ended_hours: [{ t: midnight - H, n: out({ ok: 999 }) }, { t: midnight, n: out({ ok: 7000, test: 300 }) }, { t: T - H, n: out({ ok: 566, timeout: 2 }) }],
  });
  const listed = [job(1, { node: 'build-1d', state: 'ok', ended_ms: T - 1000 })];
  const c = nodeCounts(node, listed, T, midnight);
  assert.deepEqual(c.hour, out({ ok: 120, test: 3, infra: 1 }), 'the node\'s own count, not its 30 listed jobs');
  assert.deepEqual(c.today, out({ ok: 7566, test: 300, timeout: 2 }), 'the hours from midnight on');
  const old = row({ node_id: 'local' });
  const jobs = [
    job(2, { state: 'ok', ended_ms: T - 10 * 60_000 }), job(3, { state: 'failed', exit: 101, ended_ms: T - 2 * H }),
    job(4, { state: 'failed', exit: 101, ended_ms: midnight - 1 }), job(5, { node: 'build-1d', state: 'ok', ended_ms: T - 1 }),
  ];
  const f = nodeCounts(old, jobs, T, midnight);
  assert.deepEqual([f.hour, f.today], [out({ ok: 1 }), out({ ok: 1, test: 1 })], 'a daemon without counts: from the jobs it lists');
});

test('UXF-042: a failure is a test, a timeout or the infrastructure, by its exit', () => {
  const failed = (exit: number, over: Partial<Job> = {}): Job => job(1, { state: 'failed', exit, ended_ms: T, ...over });
  assert.deepEqual([101, 1, 201, 124, 3, 125, 127].map((e) => outcomeOf(failed(e))), ['test', 'test', 'test', 'timeout', 'infra', 'infra', 'infra']);
  assert.equal(outcomeOf(failed(9, { timed_out: true })), 'timeout');
  assert.equal(outcomeOf(job(1, { state: 'ok', ended_ms: T })), 'ok');
  assert.equal(outcomeOf(job(1)), null, 'a running job has no result yet');
  assert.equal(failText(out({ test: 60, timeout: 2, infra: 5 })), '60 test · 2 timeout · 5 infra');
  assert.equal(failText(out({ ok: 4, infra: 1 })), '1 infra');
  assert.equal(failText(out({ ok: 4 })), '');
  const nodes = [row({ ended_hour: out({ test: 2 }) }), row({ node_id: 'build-1d', kind: 'Remote', ended_hour: out({ infra: 4, timeout: 1 }) }), row({ node_id: 'old' })];
  const listed = [failed(127, { node: 'old', ended_ms: T - 60_000 }), failed(1, { node: 'local', ended_ms: T - 60_000 }), failed(1, { node: 'old', ended_ms: T - 2 * H })];
  assert.deepEqual(failedHour(nodes, listed, T), out({ test: 2, timeout: 1, infra: 5 }), 'each node\'s own count; the list for a node without one');
  const unknown = [...listed, failed(101, { node: 'gone-node', ended_ms: T - 60_000 })];
  assert.deepEqual(failedHour(nodes, unknown, T), out({ test: 3, timeout: 1, infra: 5 }), 'and the list for a node no row names');
});

test('UXF-043: the whole host\'s CPUs and load, named as the whole host', () => {
  assert.equal(hostLine(row({ cpus: 20, cpu_busy_milli: 0, host_cpu_milli: 4123, load1: 5.31 })), 'whole host 4.1 of 20 CPUs · load 5.3');
  assert.equal(hostLine(row({ cpus: 20, host_cpu_milli: 4123 })), 'whole host 4.1 of 20 CPUs');
  assert.equal(hostLine(row({ cpus: 20 })), null, 'a daemon that predates it');
});
