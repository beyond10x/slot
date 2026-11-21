import type { Event } from '../generated/types.ts';

export type Tick = Extract<Event, { type: 'tick' }>;

/** One second of the cores chart: host CPU in cores, job cores per toolchain, jobs waiting for slots. */
export interface Sample {
  t: number;
  sys: number;
  by: Record<string, number>;
  waiting: number;
}

/** Census averages over the n one-second observations beginning at t. */
export interface RecordedBucket {
  t: number;
  n: number;
  host_cores: number;
  waiting: number;
}

/** Invalid or missing measurements leave holes in the chart. */
export function parseRecordedHistory(data: unknown): RecordedBucket[] {
  if (!Array.isArray(data)) return [];
  const buckets = data.filter((b): b is RecordedBucket =>
    b !== null && typeof b === 'object' && Number.isSafeInteger(b.t) &&
    Number.isSafeInteger(b.n) && b.n > 0 && b.n <= 10 &&
    Number.isFinite(b.host_cores) && b.host_cores >= 0 && Number.isFinite(b.waiting) && b.waiting >= 0);
  return mergeByT([], buckets.sort((a, b) => a.t - b.t));
}

/** A bucket describes its full interval. An exact tick in it takes priority over that mean. */
export function recordedWindow(buckets: readonly RecordedBucket[], exact: readonly { t: number }[], from: number, to: number): RecordedBucket[] {
  let i = 0;
  return buckets.filter((b) => {
    const end = b.t + b.n * 1000;
    if (end <= from || b.t >= to) return false;
    while (i < exact.length && exact[i].t < b.t) i++;
    return i === exact.length || exact[i].t >= end;
  });
}

/**
 * Merges two lists sorted by `t` into one sorted list with one entry per `t`
 * (the `newer` list wins a tie), keeping at most the last `max` entries.
 */
export function mergeByT<T extends { t: number }>(older: readonly T[], newer: readonly T[], max = Infinity): T[] {
  const out: T[] = [];
  let i = 0;
  let k = 0;
  while (i < older.length || k < newer.length) {
    const a = older[i];
    const b = newer[k];
    let next: T;
    if (b === undefined || (a !== undefined && a.t < b.t)) {
      next = a;
      i++;
    } else {
      if (a !== undefined && a.t === b.t) i++;
      next = b;
      k++;
    }
    const last = out[out.length - 1];
    if (last !== undefined && last.t === next.t) out[out.length - 1] = next;
    else out.push(next);
  }
  return out.length > max ? out.slice(out.length - max) : out;
}

/** Ticks from a history response: only well-formed ticks, sorted by `t`, one per `t`. */
export function parseHistory(data: unknown): Tick[] {
  if (!Array.isArray(data)) return [];
  const ticks = data.filter(
    (x): x is Tick => typeof x === 'object' && x !== null && (x as { type?: unknown }).type === 'tick'
      && typeof (x as { t?: unknown }).t === 'number' && Array.isArray((x as { jobs?: unknown }).jobs)
      && typeof (x as { census?: unknown }).census === 'object' && (x as { census?: unknown }).census !== null,
  );
  ticks.sort((a, b) => a.t - b.t);
  return mergeByT([], ticks);
}

/** The cores-chart sample of a tick; `toolchainOf` names a job's toolchain, `cores` is the host CPU count. */
export function coresSample(tick: Tick, toolchainOf: (id: number) => string | undefined, cores: number): Sample {
  const by: Record<string, number> = {};
  for (const l of tick.jobs) {
    const tc = toolchainOf(l.id) ?? 'other';
    by[tc] = (by[tc] ?? 0) + l.cores;
  }
  return { t: tick.t, sys: tick.sys_cpu * cores, by, waiting: tick.waiting };
}
