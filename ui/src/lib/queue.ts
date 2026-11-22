import type { Job, Priority } from '../generated/types.ts';
import { isActive, jobId, times } from './job.ts';

const RANK: Record<Priority, number> = { urgent: 0, high: 1, normal: 2, low: 3 };

/** A job's queue level; `normal` from a daemon that predates levels. */
export function level(j: Job): Priority {
  return j.priority ?? 'normal';
}

/** The level as the queue shows it: `high · waited` for a job raised because it waited. */
export function levelText(j: Job): string {
  return j.promoted ? `${level(j)} · waited` : level(j);
}

/**
 * Queued jobs in the order the pool serves them: urgent, high, normal, then low,
 * oldest request first inside a level, id breaks ties.
 */
export function servedQueue(list: Iterable<Job>): Job[] {
  const q: Job[] = [];
  for (const j of list) if (j.state === 'queued') q.push(j);
  return q.sort(
    (a, b) => RANK[level(a)] - RANK[level(b)] || times(a).queued - times(b).queued || jobId(a) - jobId(b),
  );
}

/** What a queued job waits for: a slot, or what the daemon's admission names in `wait_reason`. */
export function waitsFor(j: Job): 'disk space' | 'memory' | 'slots' {
  if (!j.wait_reason) return 'slots';
  return j.wait_reason.includes('filesystem') ? 'disk space' : 'memory';
}

/** The daemon's reason as a line of text: "waits for disk space: free space … is below 25 GB"; empty for a job that waits for a slot. */
export function waitReasonText(j: Job): string {
  return j.wait_reason ? `waits for ${waitsFor(j)}: ${j.wait_reason}` : '';
}

/** A queued job's place in the order the pool serves, 1 first, and the queue's length; null when it is not queued. */
export function queuePlace(queue: readonly Job[], id: number): { pos: number; of: number } | null {
  const k = queue.findIndex((j) => jobId(j) === id);
  return k < 0 ? null : { pos: k + 1, of: queue.length };
}

/**
 * The Queue's heading. It names what the listed jobs wait for, slots last, so it says
 * "waiting for slots" only when no listed job waits for disk space or memory.
 */
export function queueHeading(queue: readonly Job[]): string {
  const kinds = (['disk space', 'memory', 'slots'] as const).filter((k) => queue.some((j) => waitsFor(j) === k));
  const what = kinds.length > 1 ? `${kinds.slice(0, -1).join(', ')} and ${kinds[kinds.length - 1]}` : (kinds[0] ?? 'slots');
  return `Queue: waiting for ${what}, by level, oldest first`;
}

/** Nearest-rank percentile (0 < p <= 100) of ascending values; null when empty. */
export function percentile(sorted: readonly number[], p: number): number | null {
  if (!sorted.length) return null;
  const rank = Math.ceil((p / 100) * sorted.length);
  return sorted[Math.min(sorted.length, Math.max(1, rank)) - 1];
}

/** Queue length and the longest current wait in seconds (0 when empty). */
export function queueSummary(queue: readonly Job[], now: number): { length: number; longestS: number } {
  let oldest = Infinity;
  for (const j of queue) oldest = Math.min(oldest, times(j).queued);
  return { length: queue.length, longestS: queue.length ? Math.max(0, (now - oldest) / 1000) : 0 };
}

/** Jobs that failed within the last `windowMs`, and the one that ended last (highest id on a tie). */
export function failedStats(list: Iterable<Job>, now: number, windowMs = 3_600_000): { n: number; newest: Job | undefined } {
  let n = 0;
  let newest: Job | undefined;
  let newestEnd = -Infinity;
  for (const j of list) {
    const ended = times(j).ended;
    if (j.state !== 'failed' || ended === null || ended < now - windowMs) continue;
    n++;
    if (ended > newestEnd || (ended === newestEnd && newest && jobId(j) > jobId(newest))) {
      newest = j;
      newestEnd = ended;
    }
  }
  return { n, newest };
}

export interface WaitStats {
  /** Finished jobs that got a slot, within the window. */
  n: number;
  p50: number | null;
  p90: number | null;
  /** Jobs cancelled while still queued, within the window. */
  gaveUp: number;
}

/**
 * Slot wait (started - enqueued, seconds) over jobs that finished within the
 * last `windowMs`. Jobs cancelled before they started are counted as gave-up,
 * not in the percentiles; other jobs that never started are left out.
 */
export function waitStats(list: Iterable<Job>, now: number, windowMs = 3_600_000): WaitStats {
  const waits: number[] = [];
  let gaveUp = 0;
  for (const j of list) {
    if (isActive(j.state)) continue;
    const t = times(j);
    if (t.ended === null || t.ended < now - windowMs) continue;
    if (t.started !== null) waits.push(Math.max(0, (t.started - t.queued) / 1000));
    else if (j.state === 'cancelled') gaveUp++;
  }
  waits.sort((a, b) => a - b);
  return { n: waits.length, p50: percentile(waits, 50), p90: percentile(waits, 90), gaveUp };
}
