import type { CensusSession } from '../generated/types.ts';
import { hashHue } from './colors.ts';
import type { Tick } from './history.ts';

/** Cores of one tool family, split by whether slot runs the processes. */
export interface FamilyCores {
  managed: number;
  unmanaged: number;
}

/** One second of the host panel. */
export interface HostSample {
  t: number;
  hostCores: number;
  buildCores: number;
  unmanagedCores: number;
  psiCpu: number;
  psiMemory: number;
  psiIo: number;
  recommended: number;
  tokens: number;
  byFamily: Record<string, FamilyCores>;
}

export function hostSample(tick: Tick): HostSample {
  const c = tick.census;
  const byFamily: Record<string, FamilyCores> = {};
  for (const k of c.classes) {
    const f = (byFamily[k.class] ??= { managed: 0, unmanaged: 0 });
    if (k.managed) f.managed += k.cores;
    else f.unmanaged += k.cores;
  }
  return {
    t: tick.t,
    hostCores: c.host_cores,
    buildCores: c.build_cores,
    unmanagedCores: c.unmanaged_cores,
    psiCpu: c.psi_cpu,
    psiMemory: c.psi_memory,
    psiIo: c.psi_io,
    recommended: c.recommended_tokens,
    tokens: tick.tokens,
    byFamily,
  };
}

/** Display and stacking order of the known families; unknown ones follow by name. */
export const FAMILY_ORDER: readonly string[] = [
  'rust-compile', 'rust-build-script', 'rust-test',
  'go-compile', 'go-test',
  'node-build', 'node-typecheck', 'node-lint', 'node-test', 'node-e2e',
  'link', 'native',
  'helm', 'container-build',
];

const FAMILY_COLORS: Readonly<Record<string, string>> = {
  'rust-compile': '#D55E00',
  'rust-build-script': '#E69F00',
  'rust-test': '#A6400C',
  'go-compile': '#0072B2',
  'go-test': '#56B4E9',
  'node-build': '#00694A',
  'node-typecheck': '#009E73',
  'node-lint': '#5DBB8E',
  'node-test': '#2E8540',
  'node-e2e': '#8CC63F',
  link: '#6E6E6E',
  native: '#A8A8A8',
  helm: '#7F3C8D',
  'container-build': '#8C6D31',
};

/** Stable colour of a tool family: rust warm, go blue, node green, link/native grey; unknown hashed. */
export function familyColor(f: string): string {
  return FAMILY_COLORS[f] ?? `hsl(${hashHue(f)},45%,45%)`;
}

/** Families in stacking order. */
export function familyOrder(names: Iterable<string>): string[] {
  const rank = (f: string) => {
    const k = FAMILY_ORDER.indexOf(f);
    return k < 0 ? FAMILY_ORDER.length : k;
  };
  return [...new Set(names)].sort((a, b) => rank(a) - rank(b) || a.localeCompare(b));
}

export interface Band {
  family: string;
  managed: boolean;
  y0: number;
  y1: number;
}

/** Stacked bands of one sample, bottom up: each family managed then unmanaged; empty bands left out. */
export function stack(byFamily: Readonly<Record<string, FamilyCores>>, order: readonly string[]): Band[] {
  const out: Band[] = [];
  let acc = 0;
  for (const family of order) {
    const f = byFamily[family];
    if (!f) continue;
    for (const [managed, v] of [[true, f.managed], [false, f.unmanaged]] as const) {
      if (!(v > 0)) continue;
      out.push({ family, managed, y0: acc, y1: acc + v });
      acc += v;
    }
  }
  return out;
}

/** Cores of builds outside slot above which a session is flagged "not on slot". */
export const NOT_ON_SLOT_CORES = 1;

export const notOnSlot = (s: CensusSession): boolean => s.unmanaged_cores > NOT_ON_SLOT_CORES;

/** Session title, else the first 8 characters of its id, else a placeholder. */
export function sessionLabel(s: CensusSession): string {
  return s.title || (s.session ? s.session.slice(0, 8) : '(no session)');
}

/** Sessions by cores, then unmanaged cores, then label. */
export function orderSessions(list: readonly CensusSession[]): CensusSession[] {
  return [...list].sort(
    (a, b) => b.cores - a.cores || b.unmanaged_cores - a.unmanaged_cores || sessionLabel(a).localeCompare(sessionLabel(b)),
  );
}
