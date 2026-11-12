import type { HostCensus } from '../generated/types.ts';
import type { Tick } from './history.ts';

/** Test fixture: an idle host census. */
export const census = (over: Partial<HostCensus> = {}): HostCensus => ({
  host_cores: 0, build_cores: 0, unmanaged_cores: 0, psi_cpu: 0, psi_memory: 0, psi_io: 0, load1: 0,
  classes: [], sessions: [], recommended_tokens: 20, unmanaged_smoothed: 0, ...over,
});

/** Test fixture: an idle tick at `t`. */
export function tick(t: number, over: Partial<Tick> = {}): Tick {
  return { type: 'tick', t, tokens: 20, limit: 20, headroom_percent: 0, free: 20, waiting: 0, sys_cpu: 0, job_cores: 0, jobs: [], census: census(), ...over };
}
