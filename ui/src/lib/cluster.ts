// The cluster start page: nodes, the jobs on them and the headline numbers, as pure functions.
// A node is a row of `api/nodes` (the specification's NodeList); what the row does not carry (checkouts,
// finished and failed jobs) comes from the jobs of `api/state`, each of which names its node.
import type { CacheMinute, Job, Node } from '../generated/types.ts';
import { hashHue } from './colors.ts';
import { isActive } from './job.ts';

export const LOCAL = 'local';

/** The node a job runs or ran on: a job of a daemon without nodes ran on the local one. */
export const nodeOf = (j: Job): string => j.node || LOCAL;

/** The checkout of a job as the page names it: `repo` or `repo/worktree`. */
export function checkoutOf(j: Job): string {
  const base = j.repo || j.cwd.split('/').filter(Boolean).pop() || '';
  return j.checkout?.kind === 'worktree' && j.checkout.name ? `${base}/${j.checkout.name}` : base;
}

/** Colour of a checkout's lane and mark: one hue per name, the same on every load. */
export function laneColor(name: string, dark: boolean, alpha = 1): string {
  return `hsl(${hashHue(name)} ${dark ? '62%' : '55%'} ${dark ? '64%' : '38%'} / ${alpha})`;
}

/** Reads `api/nodes`, a bare array of rows: the rows with a node id; null when the answer is no array. */
export function parseNodes(data: unknown): Node[] | null {
  if (!Array.isArray(data)) return null;
  return (data as Node[]).filter((n) => n && typeof n.node_id === 'string' && n.node_id !== '');
}

/** Busy CPUs of a node over the last poll. */
export const cpuOf = (n: Node): number => (n.cpu_busy_milli ?? 0) / 1000;

/** An RFC 3339 time of a row as Unix ms; 0 when absent or unreadable. */
export function timeMs(s: string | undefined): number {
  const t = s ? Date.parse(s) : NaN;
  return Number.isFinite(t) ? t : 0;
}

export interface LocalFallback {
  host: string;
  cores: number;
  limit: number;
  tokens: number;
  free: number;
  jobCores: number;
  diskFree: number;
}

/** The local node from `api/state` alone, for a daemon older than `api/nodes`: memory and disk size are unknown (0). */
export function localFromState(f: LocalFallback, jobs: Iterable<Job>): Node {
  const all = [...jobs];
  const slots = f.limit || f.tokens;
  return {
    node_id: LOCAL, cluster_id: f.host, kind: 'Local', state: 'Ready', cpus: f.cores, slots,
    slots_in_use: Math.max(0, slots - f.free), memory_gib: 0, disk_gib: 0,
    running_jobs: all.filter((j) => j.state === 'running').length, queued_jobs: all.filter((j) => j.state === 'queued').length,
    slot_version: '', cpu_busy_milli: Math.round(f.jobCores * 1000),
    ...(f.diskFree > 0 ? { disk_free_gib: Math.round(f.diskFree / 2 ** 30) } : {}),
  };
}

/** What the page shows of a node beside its row: its checkouts with a job now, and its finished and failed jobs the daemon still lists. */
export interface NodeJobs {
  checkouts: string[];
  done: number;
  failed: number;
}

export function nodeJobs(id: string, jobs: Iterable<Job>): NodeJobs {
  const checkouts = new Set<string>();
  let done = 0;
  let failed = 0;
  for (const j of jobs) {
    if (nodeOf(j) !== id) continue;
    if (isActive(j.state)) {
      const c = checkoutOf(j);
      if (c) checkouts.add(c);
    } else if (j.state === 'ok') done++;
    else if (j.state === 'failed') failed++;
  }
  return { checkouts: [...checkouts].sort(), done, failed };
}

/** Nodes that take or run jobs: their CPUs count as available. */
export const working = (n: Node): boolean => n.state === 'Ready' || n.state === 'Draining';

/** CPUs in use and available over the working nodes. */
export function cpuTotals(nodes: readonly Node[]): { used: number; available: number } {
  let used = 0;
  let available = 0;
  for (const n of nodes) {
    if (!working(n)) continue;
    used += cpuOf(n);
    available += n.cpus;
  }
  return { used, available };
}

/** Cost of the remote nodes so far, from their join to now, their deadline or their leaving; null without a remote node with a price. */
export function cloudCost(nodes: readonly Node[], now: number): { total: number; perHour: number } | null {
  const priced = nodes.filter((n) => n.kind === 'Remote' && (n.price_per_hour_cents ?? 0) > 0);
  if (!priced.length) return null;
  let total = 0;
  let perHour = 0;
  for (const n of priced) {
    const price = (n.price_per_hour_cents ?? 0) / 100;
    const joined = timeMs(n.joined_at);
    const ends = [now, timeMs(n.deadline), timeMs(n.left_at)].filter((t) => t > 0);
    if (joined > 0) total += (price * Math.max(0, Math.min(...ends) - joined)) / 3_600_000;
    if (n.state !== 'Gone') perHour += price;
  }
  return { total, perHour };
}

/** Share of the compiler calls of the last 24 hours that the cache answered; null without calls. */
export function cacheShare(day: CacheMinute | undefined): number | null {
  if (!day) return null;
  const all = day.hits + day.misses + day.not_cached;
  return all > 0 ? day.hits / all : null;
}

/** What is left of a node's lease: the fraction of it, the seconds and the lease in hours; null without a deadline. */
export function lifeLeft(n: Node, now: number): { frac: number; leftS: number; spanH: number } | null {
  const end = timeMs(n.deadline);
  if (end <= 0) return null;
  const span = end - timeMs(n.joined_at);
  const left = Math.max(0, end - now);
  return { frac: span > 0 ? Math.min(1, left / span) : 0, leftS: left / 1000, spanH: span / 3_600_000 };
}

/** `2:12` for hours and minutes from an hour on, `4:05` for minutes and seconds below. */
export function fmtLeft(s: number): string {
  const x = Math.max(0, Math.floor(s));
  const h = Math.floor(x / 3600);
  const m = Math.floor((x % 3600) / 60);
  return h > 0 ? `${h}:${String(m).padStart(2, '0')}` : `${m}:${String(x % 60).padStart(2, '0')}`;
}

/** `1.64 TiB` from 1024 GiB on, else `604 GiB`. */
export function fmtGib(g: number): string {
  return g >= 1024 ? `${(g / 1024).toFixed(2)} TiB` : `${g} GiB`;
}

/** One line naming the machine: CPUs, slots, memory, disk; type, location and price of a remote machine. */
export function specLine(n: Node): string {
  const parts = [`${n.cpus} ${n.kind === 'Remote' ? 'vCPU' : 'cores'}`, `${n.slots} slots`];
  if (n.memory_gib > 0) parts.push(`${n.memory_gib} GiB`);
  if (n.disk_gib > 0) parts.push(`${fmtGib(n.disk_gib)} disk`);
  if (n.machine_type) parts.push(n.machine_type);
  if (n.location) parts.push(n.location);
  if (n.price_per_hour_cents) parts.push(`${(n.price_per_hour_cents / 100).toFixed(2)}/h`);
  return parts.join(' · ');
}

/** The state as the page writes it. */
export function stateText(n: Node): string {
  if (n.state === 'Draining') return 'draining · no new jobs';
  return n.state.toLowerCase();
}

/** Slot cells of a node: one per slot, the toolchain and job of the running job that holds it, else null. */
export function slotCells(slots: number, running: readonly Job[]): Array<{ tool: string; id: number } | null> {
  const cells: Array<{ tool: string; id: number } | null> = new Array(slots).fill(null);
  let k = 0;
  for (const j of [...running].sort((a, b) => Number(a.started_ms) - Number(b.started_ms) || Number(a.id) - Number(b.id))) {
    for (let t = 0; t < j.tokens && k < slots; t++) cells[k++] = { tool: j.toolchain, id: Number(j.id) };
  }
  return cells;
}

/** Jobs of the list, newest first; a node filter keeps that node's. */
export function jobList(all: Iterable<Job>, filter: string | null, max: number): Job[] {
  const out = [...all].filter((j) => filter === null || nodeOf(j) === filter);
  out.sort((a, b) => Number(b.id) - Number(a.id));
  return out.slice(0, max);
}
