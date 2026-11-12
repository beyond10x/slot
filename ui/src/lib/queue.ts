import type { Job } from '../generated/types.ts';
import { isActive, jobId, times } from './job.ts';

/** Queued jobs in the order the pool serves them: oldest request first, id breaks ties. */
export function fifoQueue(list: Iterable<Job>): Job[] {
  const q: Job[] = [];
  for (const j of list) if (j.state === 'queued') q.push(j);
  return q.sort((a, b) => times(a).queued - times(b).queued || jobId(a) - jobId(b));
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
