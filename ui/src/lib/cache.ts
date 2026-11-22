import type { CacheMinute } from '../generated/types.ts';

/** The daemon reads each compile cache server once a minute: an element counts the calls of the minute before its `t`. */
export const MINUTE_MS = 60_000;
/** The newest element is "now" while it is at most this old: one reading and one request for it may be late. */
export const FRESH_MS = 3 * MINUTE_MS;

/** One minute of the compile cache chart, in percent; `NaN` where there was no call to take a share of. */
export interface CachePoint {
  t: number;
  /** Calls answered from the cache, of all compiler calls. */
  all: number;
  /** Calls answered from the cache, of the calls the cache could have answered (hits and misses). */
  cacheable: number;
}

const count = (v: unknown): number => (typeof v === 'number' && Number.isFinite(v) && v > 0 ? v : 0);

/** Elements of an `api/cache-history` answer: well-formed ones, sorted by `t`, one per `t`; a count the daemon left out is 0. */
export function parseCacheHistory(data: unknown): CacheMinute[] {
  if (!Array.isArray(data)) return [];
  const byT = new Map<number, CacheMinute>();
  for (const x of data as unknown[]) {
    if (typeof x !== 'object' || x === null) continue;
    const m = x as Partial<Record<keyof CacheMinute, unknown>>;
    if (typeof m.t !== 'number' || !Number.isSafeInteger(m.t) || m.t <= 0) continue;
    const reasons: Record<string, number> = {};
    if (typeof m.reasons === 'object' && m.reasons !== null) {
      for (const [reason, n] of Object.entries(m.reasons)) if (count(n) > 0) reasons[reason] = count(n);
    }
    byT.set(m.t, { t: m.t, hits: count(m.hits), misses: count(m.misses), not_cached: count(m.not_cached), reasons });
  }
  return [...byT.values()].sort((a, b) => a.t - b.t);
}

/** `part` of `whole` in percent; `NaN` when `whole` is 0. */
const share = (part: number, whole: number): number => (whole > 0 ? (part / whole) * 100 : NaN);

export function cachePoint(m: CacheMinute): CachePoint {
  return { t: m.t, all: share(m.hits, m.hits + m.misses + m.not_cached), cacheable: share(m.hits, m.hits + m.misses) };
}

/** The elements whose minute reaches into [from, to]. */
export function cacheWindow(minutes: readonly CacheMinute[], from: number, to: number): CacheMinute[] {
  return minutes.filter((m) => m.t > from && m.t - MINUTE_MS < to);
}

/** The newest element, while it is fresh at `now`; a cache nobody called since then has no value now. */
export function latestMinute(minutes: readonly CacheMinute[], now: number): CacheMinute | undefined {
  const m = minutes[minutes.length - 1];
  return m !== undefined && now - m.t <= FRESH_MS ? m : undefined;
}

/** A share in the legend: a dash where there was no call, never "0 %" for it. */
export function percentOrDash(v: number | undefined): string {
  return v === undefined || !Number.isFinite(v) ? '–' : `${v.toFixed(0)} %`;
}

/** What stands in place of the lines: nothing was recorded, or nothing was compiled in the window shown. */
export function emptyText(recorded: number, inWindow: number): string {
  if (recorded === 0) return 'no cache data yet';
  return inWindow === 0 ? 'no compiler calls in this window' : '';
}

/** The newest minute in words, with the most frequent reasons a call was not cacheable; empty without a fresh minute. */
export function describeMinute(m: CacheMinute | undefined, top = 3): string {
  if (!m) return '';
  const calls = m.hits + m.misses + m.not_cached;
  if (calls === 0) return 'last minute: no compiler call';
  const reasons = Object.entries(m.reasons)
    .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
    .slice(0, top)
    .map(([reason, n]) => `${reason} ${n}`);
  const why = reasons.length ? ` (${reasons.join(', ')})` : '';
  return `last minute: ${calls} compiler calls, ${m.hits} from the cache, ${m.misses} compiled after a miss, ${m.not_cached} not cacheable${why}`;
}
