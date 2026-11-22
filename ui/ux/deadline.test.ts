// The step deadline: a command over its budget is stopped with everything it started, one in time keeps its exit code.
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { OVER_BUDGET, run, verdict } from './deadline.ts';
import type { Outcome } from './deadline.ts';

const script = fileURLToPath(new URL('./deadline.ts', import.meta.url));
const deadline = (...args: string[]) =>
  spawnSync(process.execPath, ['--disable-warning=ExperimentalWarning', script, ...args], { encoding: 'utf8', timeout: 20_000 });

/** True once `pid` is no running process: gone, or ended and not collected by its parent yet. */
async function gone(pid: number): Promise<boolean> {
  for (let n = 0; n < 100; n++) {
    let state = '';
    try {
      const st = readFileSync(`/proc/${pid}/stat`, 'utf8');
      state = st.slice(st.lastIndexOf(')') + 2, st.lastIndexOf(')') + 3);
    } catch {
      return true;
    }
    if (state === 'Z' || state === 'X') return true;
    await new Promise((ok) => setTimeout(ok, 20));
  }
  return false;
}

/** Runs a shell script under a deadline of 0.3 s; the script writes the pid of a child it starts to \$PID_FILE. */
async function stopped(sh: string): Promise<{ outcome: Outcome; childGone: boolean }> {
  const dir = mkdtempSync(join(tmpdir(), 'deadline-'));
  try {
    const pidFile = join(dir, 'child.pid');
    const outcome = await run(0.3, 'sh', ['-c', `PID_FILE=${pidFile}; ${sh}`], 200);
    const child = Number(readFileSync(pidFile, 'utf8'));
    assert.ok(child > 0);
    return { outcome, childGone: await gone(child) };
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

test('a command that outlives its deadline is stopped, the step fails and says how long it ran', { timeout: 5_000 }, () => {
  const r = deadline('0.4', 'sleep', '60');
  assert.equal(r.status, OVER_BUDGET);
  assert.match(r.stdout, /^deadline: the step was stopped after \d+\.\d s, over its budget of 0\.4 s$/m);
});

test('the children of a stopped command are stopped with it', { timeout: 5_000 }, async () => {
  // The shell starts a child that would outlive it by a minute, and waits.
  const r = await stopped('sleep 60 & echo $! > $PID_FILE; wait');
  assert.deepEqual({ code: r.outcome.code, over: r.outcome.over }, { code: OVER_BUDGET, over: true });
  assert.equal(r.childGone, true, 'the child of the stopped command is gone');
});

// Playwright starts its web server this way; a signal to the group does not reach it.
test('a child that left the process group of the command is stopped too', { timeout: 5_000 }, async () => {
  const r = await stopped('setsid sleep 60 & echo $! > $PID_FILE; wait');
  assert.equal(r.outcome.code, OVER_BUDGET);
  assert.equal(r.childGone, true, 'the child in a session of its own is gone');
});

test('a command that ignores the first signal is killed after the grace time', { timeout: 5_000 }, async () => {
  const o = await run(0.3, 'sh', ['-c', "trap '' INT; sleep 60"], 200);
  assert.deepEqual({ code: o.code, over: o.over }, { code: OVER_BUDGET, over: true });
  assert.ok(o.seconds >= 0.5 && o.seconds < 4, `stopped after ${o.seconds} s`);
});

test('a cancel of the step stops the command and its children', { timeout: 5_000 }, async () => {
  const dir = mkdtempSync(join(tmpdir(), 'deadline-'));
  try {
    const pidFile = join(dir, 'child.pid');
    const step = spawn(process.execPath, ['--disable-warning=ExperimentalWarning', script, '60', 'sh', '-c', `sleep 60 & echo $! > ${pidFile}; wait`], { stdio: 'ignore' });
    const closed = new Promise<number | null>((ok) => step.on('close', (code) => ok(code)));
    while (!existsSync(pidFile) || readFileSync(pidFile, 'utf8') === '') await new Promise((ok) => setTimeout(ok, 20));
    const child = Number(readFileSync(pidFile, 'utf8'));
    step.kill('SIGTERM');
    assert.notEqual(await closed, 0);
    assert.equal(await gone(child), true, 'the child of the cancelled command is gone');
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('a command that ends in time passes its exit code through, with the time it took', { timeout: 5_000 }, () => {
  const ok = deadline('5', 'sh', '-c', 'echo done');
  assert.equal(ok.status, 0);
  assert.match(ok.stdout, /^done\ndeadline: the step took \d+\.\d s of its 5 s \(exit 0\)$/m);
  const failed = deadline('5', 'sh', '-c', 'exit 7');
  assert.equal(failed.status, 7);
  assert.match(failed.stdout, /took \d+\.\d s of its 5 s \(exit 7\)/);
});

test('without a budget or a command it prints its usage', { timeout: 5_000 }, () => {
  assert.equal(deadline().status, 2);
  assert.equal(deadline('0', 'true').status, 2);
});

test('the last line names the time and the budget', () => {
  assert.equal(verdict({ code: 0, seconds: 61.26, over: false }, 120), 'deadline: the step took 61.3 s of its 120 s (exit 0)');
  assert.equal(verdict({ code: OVER_BUDGET, seconds: 120.04, over: true }, 120), 'deadline: the step was stopped after 120.0 s, over its budget of 120 s');
});
