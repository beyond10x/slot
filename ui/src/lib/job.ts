import type { Job, JobState, LiveJob } from '../generated/types.ts';
import { fmtBytes, fmtDur } from './format.ts';

/** The job's id: the daemon's own, or for a row a node reported (`from_node`) the id that node gave it. */
export const jobId = (j: { id: number }): number => Number(j.id);

type Named = { id: number; node?: string; from_node?: boolean };

/** What names one job in the page: its id, with the node for a node's own row, whose ids may repeat a local one. */
export const jobKey = (j: Named): string => (j.from_node ? `${j.node}/${jobId(j)}` : String(jobId(j)));

/** The id a person reads in a table cell. */
export const idLabel = (j: Named): string => String(jobId(j));

/** The build node a job runs or ran on; empty for this host (`local`, or no node in an older job log line). */
export const remoteNode = (j: { node?: string }): string => (j.node && j.node !== 'local' ? j.node : '');

/** The job named in words: `7089 on build-1d` for a node's own job, the id alone for this daemon's. */
export const jobName = (j: Named): string => (j.from_node ? `${jobId(j)} on ${j.node}` : String(jobId(j)));

export const isActive = (s: JobState): boolean => s === 'queued' || s === 'running';

export const cmdText = (j: Job): string => j.argv.join(' ');

/** Identify the checkout from recorded Git metadata. Older jobs only recorded a guessed name. */
export function checkoutLabel(j: Job, separator = ' · '): string {
  switch (j.checkout?.kind) {
    case 'repository': return j.repo;
    case 'worktree': return `${j.repo}${separator}worktree ${j.checkout.name}`;
    case 'directory': return 'No repository';
    default: return j.repo ? `Unverified · ${j.repo}` : 'Repository unknown';
  }
}

/** Label drawn inside a wide timeline bar. */
export const barLabel = (j: Job): string => `${checkoutLabel(j)}: ${j.argv.slice(0, 4).join(' ')}`;

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

/** A job's result in words: "ok", "cancelled", and "failed (101)" with the exit code. */
export const resultText = (j: Job): string => (j.state === 'failed' ? `failed (${j.exit})` : j.state);

/** The result as the output pane words it, with the exit code named: "failed (exit 101)", "ok (exit 0)", "cancelled". */
export const resultLong = (j: Job): string => (j.state === 'cancelled' ? 'cancelled' : `${j.state} (exit ${j.exit})`);

/**
 * Active jobs by id ascending; the `limit` finished jobs that ended last, newest first, id descending on a tie. A node numbers
 * its jobs on its own, so the id says nothing about when a node's job ended beside this host's.
 */
export function splitJobs(list: Iterable<Job>, limit = 60): { active: Job[]; done: Job[] } {
  const active: Job[] = [];
  const done: Job[] = [];
  for (const j of list) (isActive(j.state) ? active : done).push(j);
  active.sort((a, b) => jobId(a) - jobId(b));
  done.sort((a, b) => Number(b.ended_ms) - Number(a.ended_ms) || jobId(b) - jobId(a));
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
    `#${jobName(j)} ${j.toolchain} ${checkoutLabel(j)}`,
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
