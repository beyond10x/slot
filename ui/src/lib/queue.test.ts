import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { Job } from '../generated/types.ts';
import { jobId } from './job.ts';
import { fifoQueue, percentile, queueSummary, waitStats } from './queue.ts';

function job(over: Partial<Job>): Job {
  return {
    id: 1, argv: ['cargo', 'build'], cwd: '/w', repo: 'r', session: '', client: '', toolchain: 'rust', remote: false,
    tokens: 1, state: 'queued', enqueued_ms: 1000, started_ms: 0, ended_ms: 0, exit: 0, cpu_s: 0,
    compile_server_cpu_s: 0, peak_rss: 0, wait_reason: '', target_dir: '', ...over,
  };
}

const H = 3_600_000;

test('queue holds only queued jobs, oldest request first', () => {
  const q = fifoQueue([
    job({ id: 1, enqueued_ms: 500, state: 'running', started_ms: 600 }),
    job({ id: 2, enqueued_ms: 3000 }),
    job({ id: 3, enqueued_ms: 1000 }),
    job({ id: 4, enqueued_ms: 100, state: 'ok', started_ms: 200, ended_ms: 300 }),
  ]);
  assert.deepEqual(q.map(jobId), [3, 2]);
});

test('equal request times are served by id', () => {
  const q = fifoQueue([job({ id: 9, enqueued_ms: 1000 }), job({ id: 7, enqueued_ms: 1000 })]);
  assert.deepEqual(q.map(jobId), [7, 9]);
});

test('percentile of nothing is null', () => {
  assert.equal(percentile([], 50), null);
});

test('nearest-rank percentiles', () => {
  const v = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
  assert.equal(percentile(v, 50), 5);
  assert.equal(percentile(v, 90), 9);
  assert.equal(percentile(v, 100), 10);
  assert.equal(percentile([7], 90), 7);
  assert.equal(percentile([1, 2], 1), 1);
});

test('queue summary: length and age of the oldest waiter', () => {
  assert.deepEqual(queueSummary([], 9000), { length: 0, longestS: 0 });
  const q = [job({ id: 2, enqueued_ms: 4000 }), job({ id: 1, enqueued_ms: 1000 })];
  assert.deepEqual(queueSummary(q, 9000), { length: 2, longestS: 8 });
});

test('wait stats use started minus enqueued of finished jobs', () => {
  const now = 10 * H;
  const s = waitStats([
    job({ id: 1, state: 'ok', enqueued_ms: now - 60_000, started_ms: now - 58_000, ended_ms: now - 1000 }),
    job({ id: 2, state: 'failed', enqueued_ms: now - 60_000, started_ms: now - 50_000, ended_ms: now - 1000, exit: 1 }),
    job({ id: 3, state: 'running', enqueued_ms: now - 60_000, started_ms: now - 1000 }),
  ], now);
  assert.deepEqual(s, { n: 2, p50: 2, p90: 10, gaveUp: 0 });
});

test('jobs cancelled while queued count as gave up, not in percentiles', () => {
  const now = 10 * H;
  const s = waitStats([
    job({ id: 1, state: 'cancelled', enqueued_ms: now - 90_000, ended_ms: now - 1000, exit: -1 }),
    job({ id: 2, state: 'cancelled', enqueued_ms: now - 90_000, started_ms: now - 80_000, ended_ms: now - 1000, exit: -1 }),
  ], now);
  assert.deepEqual(s, { n: 1, p50: 10, p90: 10, gaveUp: 1 });
});

test('jobs that ended before the window are left out', () => {
  const now = 10 * H;
  const s = waitStats([
    job({ id: 1, state: 'ok', enqueued_ms: now - 2 * H, started_ms: now - 2 * H + 5000, ended_ms: now - H - 1 }),
    job({ id: 2, state: 'cancelled', enqueued_ms: now - 2 * H, ended_ms: now - H - 1, exit: -1 }),
  ], now);
  assert.deepEqual(s, { n: 0, p50: null, p90: null, gaveUp: 0 });
});

test('a failed job that never started is neither a wait nor a give-up', () => {
  const now = 10 * H;
  const s = waitStats([job({ id: 1, state: 'failed', enqueued_ms: now - 5000, ended_ms: now - 4000, exit: 127 })], now);
  assert.deepEqual(s, { n: 0, p50: null, p90: null, gaveUp: 0 });
});
