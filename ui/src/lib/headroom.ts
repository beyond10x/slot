import type { HeadroomBase, OutsideOwner } from '../generated/types.ts';

/** Largest headroom the daemon accepts, percent (`limits::MAX_HEADROOM`). */
export const MAX_HEADROOM = 50;

/** Slider step, percent. */
export const HEADROOM_STEP = 5;

/** Tokens that circulate with `headroom` percent kept free; mirrors `limits::effective_tokens`. */
export function effectiveTokens(tokens: number, headroom: number): number {
  const h = Math.min(Math.max(0, Math.round(headroom)), MAX_HEADROOM);
  return Math.max(1, Math.floor((tokens * (100 - h) + 50) / 100));
}

/** Cores builds may use together under the CPU quota for `headroom`. */
export function buildCores(cores: number, headroom: number): number {
  const h = Math.min(Math.max(0, Math.round(headroom)), MAX_HEADROOM);
  return (cores * (100 - h)) / 100;
}

/**
 * The slider's caption, for example "builds may use 14 of 20 cores, 14 of 20 slots"; on the
 * machine base the cores are the whole machine's.
 */
export function headroomCaption(cores: number, tokens: number, headroom: number, base: HeadroomBase = 'pool'): string {
  const c = buildCores(cores, headroom);
  const shown = Number.isInteger(c) ? String(c) : c.toFixed(1);
  const slots = `${effectiveTokens(tokens, headroom)} of ${tokens} slots`;
  if (base === 'machine') return `the machine may use ${shown} of ${cores} cores, builds ${slots}`;
  return `builds may use ${shown} of ${cores} cores, ${slots}`;
}

/** The machine numbers a state or a tick carries. */
export interface MachineNumbers {
  headroom_base: HeadroomBase;
  busy_cores: number;
  target_cores: number;
  outside_cores: number;
  outside: OutsideOwner[];
  quota_floor: boolean;
  over_target: boolean;
}

/** The numbers of a daemon on the pool base, or older than 0.16. */
export const POOL_BASE: MachineNumbers = {
  headroom_base: 'pool', busy_cores: 0, target_cores: 0, outside_cores: 0, outside: [], quota_floor: false, over_target: false,
};

/** The machine numbers of a state or a tick; a field an older daemon leaves out reads as the pool base's. */
export function machineNumbers(x: Partial<MachineNumbers>): MachineNumbers {
  return {
    headroom_base: x.headroom_base ?? 'pool', busy_cores: x.busy_cores ?? 0, target_cores: x.target_cores ?? 0,
    outside_cores: x.outside_cores ?? 0, outside: x.outside ?? [], quota_floor: x.quota_floor ?? false, over_target: x.over_target ?? false,
  };
}

/** The `slot status` machine line; null on the pool base or with no headroom. */
export function machineLine(m: MachineNumbers, cores: number, quotaPercent: number | null): string | null {
  if (m.headroom_base !== 'machine' || !(m.target_cores > 0)) return null;
  let quota = 'not applied';
  if (quotaPercent !== null) quota = (quotaPercent / 100).toFixed(1) + (m.quota_floor ? ' (floor)' : '');
  const owners = m.outside.map((o) => `${o.name} ${o.cores.toFixed(1)}`).join(', ');
  return (
    `machine${m.over_target ? ' (over target)' : ''}: ${m.busy_cores.toFixed(1)} of ${cores} CPUs busy, ` +
    `target ${m.target_cores.toFixed(1)}, pool quota ${quota}, ` +
    `outside ${m.outside_cores.toFixed(1)}${owners ? `: ${owners}` : ''}`
  );
}

/**
 * Error text for an answer from `PUT api/limits` that is not JSON, or null
 * when it is. A daemon older than 0.3.0 has no such path and answers with
 * this page (200, text/html).
 */
export function notJsonError(contentType: string | null): string | null {
  if (contentType?.trim().toLowerCase().startsWith('application/json')) return null;
  return 'the daemon is older than this page and has no headroom; upgrade it (systemctl --user reload slot)';
}

/** One-click headroom values: none, after a call, during a call. */
export const HEADROOM_PRESETS: readonly number[] = [0, 20, 40];
