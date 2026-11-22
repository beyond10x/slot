// The wall-time budget of a gate step: node ux/deadline.ts <seconds> <command> [args…]
// Runs the command in its own process group, prints the time it took, and passes its exit code
// through. The command is the step's own shell, not a nested `task`: Task copies the output of
// what it runs, and what a stopped test runner prints after Task left is lost. At the deadline the group gets SIGINT, so a test runner ends its run and prints its
// report; after the grace time the group and every process the command started are killed, also
// those that left the group (Playwright starts its web server in a session of its own). Exit 124.
import { spawn } from 'node:child_process';
import { readdirSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

/** Exit code of a step stopped at its deadline, as timeout(1) gives. */
export const OVER_BUDGET = 124;
/** Set for the command: the time of its deadline in ms since the epoch, so a test runner can report before it is stopped. */
export const DEADLINE_AT = 'STEP_DEADLINE_AT';
/** Time between the first signal and SIGKILL: below the 5 s a cancelled job gets from the pool. */
const GRACE_MS = 3_000;

export interface Outcome {
  /** The command's exit code; `OVER_BUDGET` when it was stopped at the deadline. */
  code: number;
  seconds: number;
  over: boolean;
}

/** One line for the end of the step: how long it ran, against which budget. */
export const verdict = (o: Outcome, budget_s: number): string =>
  o.over
    ? `deadline: the step was stopped after ${o.seconds.toFixed(1)} s, over its budget of ${budget_s} s`
    : `deadline: the step took ${o.seconds.toFixed(1)} s of its ${budget_s} s (exit ${o.code})`;

/** State, parent and start time of a process from `/proc/<pid>/stat`; undefined when it is gone. */
function stat(pid: number): { state: string; ppid: number; start: string } | undefined {
  try {
    const s = readFileSync(`/proc/${pid}/stat`, 'utf8');
    // The fields after the command name, which may hold spaces and brackets: state, parent, …, start time.
    const f = s.slice(s.lastIndexOf(')') + 2).split(' ');
    return { state: f[0], ppid: Number(f[1]), start: f[19] };
  } catch {
    return undefined;
  }
}

/** True while the process `pid` that started at `start` runs: not gone, not replaced, not a zombie. */
function running(pid: number, start: string): boolean {
  const st = stat(pid);
  return st !== undefined && st.start === start && st.state !== 'Z' && st.state !== 'X';
}

/** `root` and every live process below it, each with the start time that tells it from a later process of the same pid. */
export function family(root: number): Map<number, string> {
  const children = new Map<number, number[]>();
  const starts = new Map<number, string>();
  let pids: number[] = [];
  try {
    pids = readdirSync('/proc').map(Number).filter((n) => Number.isInteger(n) && n > 0);
  } catch {
    /* no /proc: only the process group is stopped */
  }
  for (const pid of pids) {
    const st = stat(pid);
    if (!st) continue;
    starts.set(pid, st.start);
    children.set(st.ppid, [...(children.get(st.ppid) ?? []), pid]);
  }
  const out = new Map<number, string>();
  const todo = [root];
  for (let pid = todo.pop(); pid !== undefined; pid = todo.pop()) {
    const start = starts.get(pid);
    if (start === undefined || out.has(pid)) continue;
    out.set(pid, start);
    todo.push(...(children.get(pid) ?? []));
  }
  return out;
}

export function run(budget_s: number, command: string, args: string[], grace_ms = GRACE_MS): Promise<Outcome> {
  const started = performance.now();
  // detached: the child leads a new process group, so one signal reaches what it started.
  const env = { ...process.env, [DEADLINE_AT]: String(Date.now() + budget_s * 1000) };
  const child = spawn(command, args, { stdio: 'inherit', detached: true, env });
  const group = (signal: NodeJS.Signals): void => {
    try {
      if (child.pid !== undefined) process.kill(-child.pid, signal);
    } catch {
      /* the group is gone */
    }
  };
  // What the command started, seen while its parents live: an orphan cannot be found later.
  const seen = new Map<number, string>();
  const look = (): void => {
    if (child.pid !== undefined) for (const [pid, start] of family(child.pid)) seen.set(pid, start);
  };
  const killAll = (): void => {
    look();
    group('SIGKILL');
    for (const [pid, start] of seen) {
      if (!running(pid, start)) continue;
      try {
        process.kill(pid, 'SIGKILL');
      } catch {
        /* gone since */
      }
    }
  };
  let over = false;
  let stopping = false;
  let hard: NodeJS.Timeout | undefined;
  const stop = (signal: NodeJS.Signals): void => {
    stopping = true;
    look();
    group(signal);
    hard ??= setTimeout(killAll, grace_ms);
  };
  const deadline = setTimeout(() => {
    over = true;
    stop('SIGINT');
  }, budget_s * 1000);
  // A cancel of this process (Ctrl-C, a cancelled job) stops the command too: it is not in the
  // canceller's process group any more.
  const forwarded: NodeJS.Signals[] = ['SIGINT', 'SIGTERM', 'SIGHUP'];
  for (const s of forwarded) process.on(s, stop);
  return new Promise((done, fail) => {
    child.on('error', (e) => {
      clearTimeout(deadline);
      fail(e);
    });
    child.on('close', (code, signal) => {
      clearTimeout(deadline);
      const end = (): void => {
        clearTimeout(hard);
        for (const s of forwarded) process.off(s, stop);
        done({ code: over ? OVER_BUDGET : (code ?? (signal ? 1 : 0)), seconds: (performance.now() - started) / 1000, over });
      };
      if (!stopping) return end();
      // The command is gone; what it started may still write its report (a test runner whose
      // launcher left first). It has the rest of the grace time; then nothing of it is left, so
      // no build slot is held and no port kept.
      const wait = (): void => {
        if ([...seen].some(([pid, start]) => pid !== child.pid && running(pid, start))) {
          setTimeout(wait, 50);
          return;
        }
        killAll();
        end();
      };
      wait();
    });
  });
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [seconds, command, ...args] = process.argv.slice(2);
  const budget = Number(seconds);
  if (!(budget > 0) || !command) {
    console.error('usage: node ux/deadline.ts <seconds> <command> [args…]');
    process.exit(2);
  }
  const outcome = await run(budget, command, args);
  console.log(verdict(outcome, budget));
  process.exit(outcome.code);
}
