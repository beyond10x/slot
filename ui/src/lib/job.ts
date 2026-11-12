import type { Job, JobState, LiveJob } from '../generated/types.ts';
import { fmtBytes, fmtDur } from './format.ts';

export const jobId = (j: { id: number }): number => Number(j.id);

export const isActive = (s: JobState): boolean => s === 'queued' || s === 'running';

export const cmdText = (j: Job): string => j.argv.join(' ');

/** Label drawn inside a wide timeline bar. */
export const barLabel = (j: Job): string => (j.repo ? j.repo + ': ' : '') + j.argv.slice(0, 4).join(' ');

export interface Times {
  queued: number;
  started: number | null;
  ended: number | null;
}

/** Job times in ms; unset (0) times are null. */
export function times(j: Job): Times {
  const s = Number(j.started_ms);
  const e = Number(j.ended_ms);
  return { queued: Number(j.enqueued_ms), started: s > 0 ? s : null, ended: e > 0 ? e : null };
}

/** Seconds spent waiting for a slot, up to start, end, or now. */
export function waitS(j: Job, now: number): number {
  const t = times(j);
  return ((t.started ?? t.ended ?? now) - t.queued) / 1000;
}

/** Seconds running, up to end or now; null if it never started. */
export function runS(j: Job, now: number): number | null {
  const t = times(j);
  return t.started === null ? null : ((t.ended ?? now) - t.started) / 1000;
}

/** Active jobs by id ascending; the last `limit` finished jobs by id descending. */
export function splitJobs(list: Iterable<Job>, limit = 60): { active: Job[]; done: Job[] } {
  const active: Job[] = [];
  const done: Job[] = [];
  for (const j of list) (isActive(j.state) ? active : done).push(j);
  active.sort((a, b) => jobId(a) - jobId(b));
  done.sort((a, b) => jobId(b) - jobId(a));
  return { active, done: done.slice(0, limit) };
}

/** The running job that started last (highest id on a tie), or undefined. */
export function newestRunning(list: Iterable<Job>): Job | undefined {
  let best: Job | undefined;
  for (const j of list) {
    if (j.state !== 'running') continue;
    if (!best) best = j;
    else {
      const d = Number(j.started_ms) - Number(best.started_ms);
      if (d > 0 || (d === 0 && jobId(j) > jobId(best))) best = j;
    }
  }
  return best;
}

/** Hover text for a timeline bar. */
export function tooltipText(j: Job, l: LiveJob | undefined, now: number): string {
  const run = runS(j, now);
  const lines = [
    `#${jobId(j)} ${j.toolchain} ${j.repo}`,
    cmdText(j),
    j.cwd,
    `state ${j.state}` + (isActive(j.state) ? '' : ` exit ${j.exit}`),
    `wait ${fmtDur(waitS(j, now))}` + (run === null ? '' : `  ran ${fmtDur(run)}`),
  ];
  if (l) lines.push(`cores ${l.cores.toFixed(1)}  RSS ${fmtBytes(Number(l.rss))}  procs ${l.procs}`);
  if (j.cpu_s) lines.push(`CPU ${j.cpu_s.toFixed(0)} s  peak RSS ${fmtBytes(Number(j.peak_rss))}`);
  if (j.session) lines.push(`session ${j.session}`);
  return lines.join('\n');
}
