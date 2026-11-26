// The cluster start page: nodes, the jobs on them and the headline numbers, as pure functions.
// A node is a row of `api/nodes` (the specification's NodeList); what the row does not carry (checkouts, and the
// finished jobs of a daemon that does not count them) comes from the jobs of `api/state`, each of which names its node.
import type { CacheMinute, Job, Node, Outcomes } from '../generated/types.ts';
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

/** Busy CPUs of the whole machine of a node, every program counted; the pool's from a node daemon that predates it. */
export const hostCpuOf = (n: Node): number => (n.host_cpu_milli === undefined ? cpuOf(n) : n.host_cpu_milli / 1000);

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

/** What the page shows of a node beside its row: its checkouts with a job now. */
export interface NodeJobs {
  checkouts: string[];
}

export function nodeJobs(id: string, jobs: Iterable<Job>): NodeJobs {
  const checkouts = new Set<string>();
  for (const j of jobs) {
    if (nodeOf(j) !== id || !isActive(j.state)) continue;
    const c = checkoutOf(j);
    if (c) checkouts.add(c);
  }
  return { checkouts: [...checkouts].sort() };
}

/** The result of an ended job, a failure by its cause: `infra` for a lost connection (3), a command that could not run (125) or was not found (127). */
export type Outcome = keyof Outcomes;

export const NO_OUTCOMES: Readonly<Outcomes> = { ok: 0, test: 0, timeout: 0, infra: 0, cancelled: 0 };

/** The result of job `j` as the daemon counts it (`Outcome::of`); null while it is queued or running. */
export function outcomeOf(j: Job): Outcome | null {
  switch (j.state) {
    case 'ok': return 'ok';
    case 'cancelled': return 'cancelled';
    case 'failed':
      if (j.timed_out || j.exit === 124) return 'timeout';
      return j.exit === 3 || j.exit === 125 || j.exit === 127 ? 'infra' : 'test';
    default: return null;
  }
}

/** Failed jobs of a count, every cause. */
export const failedOf = (o: Outcomes): number => o.test + o.timeout + o.infra;

/** The failures of a count by cause, `60 test · 2 timeout · 5 infra`, causes without a job left out; empty without a failure. */
export function failText(o: Outcomes): string {
  return (['test', 'timeout', 'infra'] as const).filter((k) => o[k] > 0).map((k) => `${o[k]} ${k}`).join(' · ');
}

function sum(a: Outcomes, b: Outcomes): Outcomes {
  return { ok: a.ok + b.ok, test: a.test + b.test, timeout: a.timeout + b.timeout, infra: a.infra + b.infra, cancelled: a.cancelled + b.cancelled };
}

/** The listed jobs of node `id` that ended in `[from, to]`, by result. */
function countListed(id: string, jobs: Iterable<Job>, from: number, to: number): Outcomes {
  const n = { ...NO_OUTCOMES };
  for (const j of jobs) {
    const o = outcomeOf(j);
    const end = Number(j.ended_ms);
    if (o && nodeOf(j) === id && end > from && end <= to) n[o]++;
  }
  return n;
}

/** Local midnight of the day of `now`. */
export function midnightOf(now: number): number {
  const d = new Date(now);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

/**
 * A node's finished jobs over the last 60 min and today (from `midnight`), as the node counts them in its own job log; for a
 * daemon that does not count, from the jobs the page lists.
 */
export function nodeCounts(n: Node, jobs: Iterable<Job>, now: number, midnight = midnightOf(now)): { hour: Outcomes; today: Outcomes } {
  const list = [...jobs];
  const hour = n.ended_hour ?? countListed(n.node_id, list, now - 3_600_000, now);
  const today = n.ended_hours
    ? n.ended_hours.filter((h) => h.t >= midnight).reduce((a, h) => sum(a, h.n), { ...NO_OUTCOMES })
    : countListed(n.node_id, list, midnight, now);
  return { hour, today };
}

/** Failed jobs of the last 60 min over every node: each node's own count, the listed jobs of a node without one or of no row. */
export function failedHour(nodes: readonly Node[], jobs: Iterable<Job>, now: number): Outcomes {
  const list = [...jobs];
  let n = { ...NO_OUTCOMES };
  for (const node of nodes) n = sum(n, node.ended_hour ?? countListed(node.node_id, list, now - 3_600_000, now));
  const named = new Set(nodes.map((x) => x.node_id));
  for (const id of new Set(list.map(nodeOf))) if (!named.has(id)) n = sum(n, countListed(id, list, now - 3_600_000, now));
  return { ...n, ok: 0, cancelled: 0 };
}

/** The whole machine's busy CPUs and load, every program counted: `whole host 4.1 of 20 CPUs · load 5.3`; null from a daemon that predates it. */
export function hostLine(n: Node): string | null {
  if (n.host_cpu_milli === undefined) return null;
  const cpu = `whole host ${(n.host_cpu_milli / 1000).toFixed(1)} of ${n.cpus} CPUs`;
  return n.load1 === undefined ? cpu : `${cpu} · load ${n.load1.toFixed(1)}`;
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

/**
 * Cost of the remote nodes so far: from the machine's boot (as the node reports it; else its join) to now, or to its leaving; a
 * deadline does not stop the count, a machine that outlives it is still paid. Null without a remote node with a price.
 */
export function cloudCost(nodes: readonly Node[], now: number): { total: number; perHour: number } | null {
  const priced = nodes.filter((n) => n.kind === 'Remote' && (n.price_per_hour_cents ?? 0) > 0);
  if (!priced.length) return null;
  let total = 0;
  let perHour = 0;
  for (const n of priced) {
    const price = (n.price_per_hour_cents ?? 0) / 100;
    const start = timeMs(n.booted_at) || timeMs(n.joined_at);
    const end = Math.min(now, timeMs(n.left_at) || now);
    if (start > 0) total += (price * Math.max(0, end - start)) / 3_600_000;
    if (n.state !== 'Gone') perHour += price;
  }
  return { total, perHour };
}

/** Farthest the poweroff a node reports and the deadline of the cluster file may lie apart before the card says so. */
export const LEASE_SLACK_MS = 5 * 60_000;

/** `14:00` today, `25 Nov 14:00` on another day; `short` gives the day alone on another day. */
function fmtWhen(ms: number, now = Date.now(), short = false): string {
  const d = new Date(ms);
  const hm = d.toLocaleTimeString('sv-SE', { hour: '2-digit', minute: '2-digit' });
  if (midnightOf(ms) === midnightOf(now)) return hm;
  const day = d.toLocaleDateString('en-GB', { day: 'numeric', month: 'short' });
  return short ? day : `${day} ${hm}`;
}

/**
 * Why a remote node's lease is in doubt, in a sentence and in a few words for under the lease ring: the poweroff the node has
 * scheduled lies more than 5 minutes from the deadline of the cluster file, or a node that reports its machine has none scheduled.
 * Null when they agree, without a deadline and from a node daemon that reports neither boot nor poweroff.
 */
export function leaseMismatch(n: Node, now = Date.now()): { text: string; short: string } | null {
  const deadline = timeMs(n.deadline);
  if (n.kind !== 'Remote' || deadline <= 0) return null;
  const file = `the cluster file says ${fmtWhen(deadline, now)}`;
  const off = timeMs(n.poweroff_at);
  if (off <= 0) return n.booted_at ? { text: `no poweroff scheduled on the node; ${file}`, short: 'no poweroff' } : null;
  if (Math.abs(off - deadline) <= LEASE_SLACK_MS) return null;
  return { text: `the node powers off at ${fmtWhen(off, now)}, ${file}`, short: `file ${fmtWhen(deadline, now, true)}` };
}

/** Share of the compiler calls of the last 24 hours that the cache answered; null without calls. */
export function cacheShare(day: CacheMinute | undefined): number | null {
  if (!day) return null;
  const all = day.hits + day.misses + day.not_cached;
  return all > 0 ? day.hits / all : null;
}

/**
 * What is left of a node's lease: the fraction of it, the seconds and the lease in hours; null without an end. The lease ends at the
 * poweroff the node has scheduled, else at the deadline of the cluster file; it starts at the machine's boot, else at its join.
 */
export function lifeLeft(n: Node, now: number): { frac: number; leftS: number; spanH: number } | null {
  const end = timeMs(n.poweroff_at) || timeMs(n.deadline);
  if (end <= 0) return null;
  const span = end - (timeMs(n.booted_at) || timeMs(n.joined_at));
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

/** Jobs of the list, newest queued first, then by id (a remote node's ids lie below 0); a node filter keeps that node's. */
export function jobList(all: Iterable<Job>, filter: string | null, max: number): Job[] {
  const out = [...all].filter((j) => filter === null || nodeOf(j) === filter);
  out.sort((a, b) => Number(b.enqueued_ms) - Number(a.enqueued_ms) || Number(b.id) - Number(a.id));
  return out.slice(0, max);
}
