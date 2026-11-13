// Usage of the pool per project, repository or session (`GET api/usage`).
import type { By, Usage } from '../generated/types.ts';

export type UsageWindow = 'today' | '24h' | '7d';

export interface UsageLine {
  key: string;
  label: string;
  /** Bar length, percent of the largest row. */
  width: number;
  share: string;
  cpuShare: string;
  slotHours: string;
  jobs: number;
  wait: string;
}

const HOUR = 3600_000;

export function windowStart(w: UsageWindow, now: Date): number {
  if (w === 'today') {
    return new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  }
  return now.getTime() - (w === '24h' ? 24 : 7 * 24) * HOUR;
}

export function usageUrl(w: UsageWindow, by: By, now: Date): string {
  return `api/usage?from=${windowStart(w, now)}&to=${now.getTime()}&by=${by}`;
}

const pct = (x: number): string => `${(x * 100).toFixed(1)}%`;

export function usageRows(u: Usage, top: number): UsageLine[] {
  const rows = u.rows.slice(0, top);
  const max = rows.reduce((m, r) => Math.max(m, r.slot_share), 0);
  return rows.map((r) => ({
    key: r.key,
    label: r.title || (u.by === 'session' ? r.key.slice(0, 8) : r.key) || '(none)',
    width: max > 0 ? (r.slot_share / max) * 100 : 0,
    share: pct(r.slot_share),
    cpuShare: pct(r.cpu_share),
    slotHours: (r.slot_s / 3600).toFixed(1),
    jobs: r.jobs,
    wait: `${Math.round(r.wait_p50_s)}s`,
  }));
}
