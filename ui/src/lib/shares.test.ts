import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { Job, SharesView } from '../generated/types.ts';
import { shareCell, sharesLine } from './shares.ts';

function job(over: Partial<Job>): Job {
  return {
    id: 1, argv: ['cargo', 'build'], cwd: '/w/slot', repo: 'slot', checkout: { kind: 'repository' }, session: '', client: '',
    toolchain: 'rust', remote: false, tokens: 4, state: 'running', enqueued_ms: 1000, started_ms: 2000, ended_ms: 0,
    exit: 0, cpu_s: 0, compile_server_cpu_s: 0, peak_rss: 0, wait_reason: '', target_dir: '', priority: 'normal', promoted: false, node: 'local', ...over,
  };
}

function view(over: Partial<SharesView>): SharesView {
  return {
    admission: 'shares', floor: 4, max: 48, pressure_max: 60, cpus: 20, budget: 15, cap: 17, min_budget: 10, free: 3.2, busy: 9, starting: 2.8,
    running: 12, idle: 0, lent: 0, pressure: 12, braking: false, ...over,
  };
}

test('by slots a job shows its slots and the slots it collected', () => {
  assert.deepEqual(shareCell(job({ tokens: 4 })), { text: '4', title: '' });
  assert.deepEqual(shareCell(job({ state: 'queued', tokens: 4, collected: 2 })), {
    text: '2/4', title: 'holds 2 of the 4 slots it asked for',
  });
});

test('by share a running job shows what it uses of the share it asked for', () => {
  assert.deepEqual(shareCell(job({ share: 2.25, share_used: 0.04 })), {
    text: '0.0/2.3', title: 'uses 0.04 of the 2.25 CPUs it asked for',
  });
  assert.deepEqual(shareCell(job({ share: 1 })), { text: '1.0', title: 'asked for a share of 1 CPUs' });
});

test('by share a queued job shows the share it asks for', () => {
  assert.deepEqual(shareCell(job({ state: 'queued', share: 4.03 })), { text: '4.0', title: 'asks for a share of 4.03 CPUs' });
});

test('the line names the free share, the budget and the bounds, and the brake while it holds', () => {
  assert.equal(sharesLine(null), '');
  assert.equal(sharesLine(view({ admission: 'slots' })), '');
  assert.equal(
    sharesLine(view({})),
    'by share: 3.2 of a budget of 15.0 CPUs free (cap 17.0) · 12 running (4 to 48)',
  );
  assert.equal(
    sharesLine(view({ idle: 2, lent: 2, braking: true, pressure: 72 })),
    'by share: 3.2 of a budget of 15.0 CPUs free (cap 17.0) · 12 running (4 to 48) · 2 idle lend 2.0 · CPU pressure 72 % over 60 %: only short jobs start',
  );
  assert.equal(sharesLine(view({ free: -0.4 })).startsWith('by share: 0.0 of'), true);
});
