import type { LiveJob, Unit } from '../generated/types.ts';

export type Phase = 'compiling' | 'linking' | 'testing' | 'running';

const COMPILING = new Set(['compile', 'build-script', 'native']);

/**
 * What a job is doing from its live units: compiling if any compile,
 * build-script or native unit runs; else testing if any test runs; else
 * linking if only link units run; else running.
 */
export function phase(units: readonly Unit[]): Phase {
  if (units.some((u) => COMPILING.has(u.kind))) return 'compiling';
  if (units.some((u) => u.kind === 'test')) return 'testing';
  if (units.length && units.every((u) => u.kind === 'link')) return 'linking';
  return 'running';
}

/** Chips in display order: most cores first, then longest running, then name. */
export function orderUnits(units: readonly Unit[]): Unit[] {
  return [...units].sort((a, b) => b.cores - a.cores || b.secs - a.secs || a.name.localeCompare(b.name));
}

/** Units the job has finished so far: seen minus those listed as running now. */
export function unitsDone(l: LiveJob): number {
  return Math.max(0, l.units_seen - l.units.length);
}

export interface KindStyle {
  label: string;
  color: string;
}

const KINDS: Readonly<Record<string, KindStyle>> = {
  compile: { label: 'compile', color: '#0072B2' },
  'build-script': { label: 'build.rs', color: '#E69F00' },
  test: { label: 'test', color: '#009E73' },
  native: { label: 'cc', color: '#7F3C8D' },
  link: { label: 'link', color: '#555555' },
};

/** Label and colour of a unit kind; unknown kinds keep their name in grey. */
export function kindStyle(kind: string): KindStyle {
  return KINDS[kind] ?? { label: kind, color: '#8c8c8c' };
}

/** Chip text after the kind label: `name · 1.8 cores · 12s`. */
export function unitText(u: Unit, fmtSecs: (s: number) => string): string {
  return `${u.name} · ${u.cores.toFixed(1)} cores · ${fmtSecs(u.secs)}`;
}
