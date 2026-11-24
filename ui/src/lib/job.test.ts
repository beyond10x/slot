import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { Job, LiveJob } from '../generated/types.ts';
import { barLabel, checkoutLabel, jobId, newestRunning, resultText, runS, splitJobs, times, tooltipText, waitS } from './job.ts';

function job(over: Partial<Job>): Job {
  return {
    id: 1, argv: ['cargo', 'build', '--release', '-p', 'slot'], cwd: '/w/slot', repo: 'slot', checkout: { kind: 'repository' }, session: '', client: '',
    toolchain: 'rust', remote: false, tokens: 1, state: 'queued', enqueued_ms: 1000, started_ms: 0, ended_ms: 0,
    exit: 0, cpu_s: 0, compile_server_cpu_s: 0, peak_rss: 0, wait_reason: '', target_dir: '', priority: 'normal', promoted: false, node: 'local', ...over,
  };
}

test('unset times are null', () => {
  assert.deepEqual(times(job({})), { queued: 1000, started: null, ended: null });
});

test('a queued job waits until now and has not run', () => {
  const j = job({});
  assert.equal(waitS(j, 4000), 3);
  assert.equal(runS(j, 4000), null);
});

test('a running job waited until start and runs until now', () => {
  const j = job({ state: 'running', started_ms: 2000 });
  assert.equal(waitS(j, 9000), 1);
  assert.equal(runS(j, 9000), 7);
});

test('a job cancelled while queued waited until its end', () => {
  const j = job({ state: 'cancelled', ended_ms: 6000, exit: -1 });
  assert.equal(waitS(j, 99_000), 5);
  assert.equal(runS(j, 99_000), null);
});

test('split sorts active ascending and keeps the newest finished', () => {
  const list = [
    job({ id: 3, state: 'running' }), job({ id: 1, state: 'queued' }),
    ...Array.from({ length: 70 }, (_, k) => job({ id: 10 + k, state: 'ok' })),
  ];
  const { active, done } = splitJobs(list);
  assert.deepEqual(active.map(jobId), [1, 3]);
  assert.equal(done.length, 60);
  assert.equal(jobId(done[0]), 79);
});

test('newest running is the one started last, id breaks ties, others ignored', () => {
  assert.equal(newestRunning([]), undefined);
  const list = [
    job({ id: 1, state: 'running', started_ms: 5000 }),
    job({ id: 2, state: 'running', started_ms: 9000 }),
    job({ id: 3, state: 'running', started_ms: 9000 }),
    job({ id: 4, state: 'queued' }),
    job({ id: 5, state: 'ok', started_ms: 20_000, ended_ms: 21_000 }),
  ];
  assert.equal(newestRunning(list)?.id, 3);
});

test('bar label shows repo and the first four words', () => {
  assert.equal(barLabel(job({})), 'slot: cargo build --release -p');
});

test('a subdirectory of a linked checkout shows the repository and worktree across labels', () => {
  const j = job({ repo: 'parser', cwd: '/trees/grammar/src', checkout: { kind: 'worktree', name: 'grammar' } });
  assert.equal(checkoutLabel(j, '\n'), 'parser\nworktree grammar');
  assert.equal(barLabel(j), 'parser · worktree grammar: cargo build --release -p');
  assert.match(tooltipText(j, undefined, 4000), /^#1 rust parser · worktree grammar\n/);
});

test('a directory outside Git and an older job without a recorded name have distinct labels', () => {
  const directory = job({ cwd: '/work/bench', repo: '', checkout: { kind: 'directory' } });
  const historic = job({ cwd: '/work/bench', repo: '', checkout: undefined });
  assert.equal(checkoutLabel(directory), 'No repository');
  assert.equal(checkoutLabel(historic), 'Repository unknown');
  assert.equal(barLabel(historic), 'Repository unknown: cargo build --release -p');
  assert.match(tooltipText(directory, undefined, 4000), /\n\/work\/bench\n/);
});

test('historical names stay visible and unverified in rows, timeline bars and hover details', () => {
  for (const repo of ['slot', 'impl-gate']) {
    const historic = job({ cwd: `/work/${repo}`, repo, checkout: undefined });
    const label = `Unverified · ${repo}`;
    assert.equal(checkoutLabel(historic, '\n'), label);
    assert.equal(barLabel(historic), `${label}: cargo build --release -p`);
    assert.equal(tooltipText(historic, undefined, 4000).split('\n')[0], `#1 rust ${label}`);
  }
});

test('tooltip shows exit only for finished jobs and live numbers when given', () => {
  const l: LiveJob = { id: 1, cores: 3.25, rss: 2 ** 29, procs: 4, units: [], units_seen: 0 };
  const run = tooltipText(job({ state: 'running', started_ms: 2000 }), l, 12_000);
  assert.match(run, /^#1 rust slot\n/);
  assert.match(run, /\nstate running\n/);
  assert.match(run, /\nwait 1\.0s {2}ran 10s\n/);
  assert.match(run, /\ncores 3\.3 {2}RSS 512 MB {2}procs 4$/);
  const done = tooltipText(job({ state: 'failed', started_ms: 2000, ended_ms: 3000, exit: 101, cpu_s: 12 }), undefined, 9000);
  assert.match(done, /\nstate failed exit 101\n/);
  assert.match(done, /\nCPU 12 s {2}peak RSS $/);
});

test('a finished job states its result in words, a failed one with its exit code', () => {
  assert.equal(resultText(job({ state: 'ok' })), 'ok');
  assert.equal(resultText(job({ state: 'failed', exit: 101 })), 'failed (101)');
  assert.equal(resultText(job({ state: 'cancelled', exit: -1 })), 'cancelled');
});
