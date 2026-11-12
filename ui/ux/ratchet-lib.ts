// The UX ratchet: every metric is lower-is-better; the baseline only moves down, or up with a decision id.

export interface Entry {
  value: number;
  tolerance: number;
  /** Decision id that allowed this value to get worse than on main. */
  decision?: string;
}

export interface Baseline {
  about: string;
  tolerances: Record<string, string>;
  scenarios: Record<string, Record<string, Entry>>;
}

export type Metrics = Record<string, Record<string, number>>;

/** Tolerance for a metric key: relative (fraction of the value) for keys in `relative`, else absolute. */
export interface ToleranceRule {
  absolute: Record<string, number>;
  relative: Record<string, number>;
}

export const RULES: ToleranceRule = {
  absolute: { 'layout_shift.per_minute': 0.01 },
  relative: { 'reading_load.max': 0.1, 'reading_load.median': 0.1 },
};

export function toleranceFor(key: string, value: number, rules: ToleranceRule = RULES): number {
  if (key in rules.absolute) return rules.absolute[key];
  if (key in rules.relative) return Math.ceil(value * rules.relative[key]);
  return 0;
}

const round = (v: number): number => Math.round(v * 1000) / 1000;

export interface Outcome {
  failures: string[];
  /** The baseline with every improvement taken in. */
  next: Baseline;
}

/** Keys whose value depends on the host's fonts: compared in a round, on one host, not on every merge. */
export const hostDependent = (key: string): boolean => key.startsWith('reading_load.') || key.startsWith('layout_shift.');

/**
 * Compares measured `current` against `base`; `main` is the baseline on the main branch, if known.
 * Keys for which `skip` is true are carried over unchecked.
 */
export function compare(base: Baseline, current: Metrics, main?: Baseline, skip: (key: string) => boolean = () => false): Outcome {
  const failures: string[] = [];
  const next: Baseline = { ...base, scenarios: {} };
  for (const [sc, keys] of Object.entries(base.scenarios)) {
    next.scenarios[sc] = {};
    for (const [key, e] of Object.entries(keys)) {
      if (skip(key)) {
        next.scenarios[sc][key] = e;
        continue;
      }
      const v = current[sc]?.[key];
      if (v === undefined || Number.isNaN(v)) {
        failures.push(`${sc} ${key}: missing`);
        next.scenarios[sc][key] = e;
        continue;
      }
      if (v > e.value + e.tolerance) failures.push(`${sc} ${key}: ${round(v)} is worse than the baseline ${e.value} (tolerance ${e.tolerance})`);
      if (v < e.value - e.tolerance) {
        failures.push(`${sc} ${key}: ${round(v)} improves on the baseline ${e.value}; take it in (ratchet --update)`);
      }
      next.scenarios[sc][key] = v < e.value - e.tolerance ? { value: round(v), tolerance: toleranceFor(key, v) } : e;
    }
  }
  for (const [sc, keys] of Object.entries(current)) {
    for (const [key, v] of Object.entries(keys)) {
      if (base.scenarios[sc]?.[key] !== undefined || skip(key)) continue;
      failures.push(`${sc} ${key}: measured but not in the baseline`);
      next.scenarios[sc] ??= {};
      next.scenarios[sc][key] = { value: round(v), tolerance: toleranceFor(key, v) };
    }
  }
  if (main) {
    for (const [sc, keys] of Object.entries(base.scenarios)) {
      for (const [key, e] of Object.entries(keys)) {
        if (skip(key)) continue;
        const m = main.scenarios[sc]?.[key];
        if (m && e.value > m.value + m.tolerance && !e.decision) {
          failures.push(`${sc} ${key}: baseline ${e.value} is worse than main's ${m.value} and names no decision`);
        }
      }
    }
  }
  return { failures, next };
}

/** `compare`, keeping only the failures `--update` cannot fix: worse values and worsened baselines. */
export function hardFailures(o: Outcome): string[] {
  return o.failures.filter((f) => !f.includes('take it in') && !f.includes('not in the baseline'));
}
