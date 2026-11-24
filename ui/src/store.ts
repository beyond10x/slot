import { computed, reactive, shallowReactive, shallowRef, triggerRef } from 'vue';
import type { CacheMinute, Event, HostCensus, Job, LiveJob, Node, SharesView, State } from './generated/types.ts';
import { MINUTE_MS, cachePoint, cacheWindow, latestMinute, parseCacheDay, parseCacheHistory } from './lib/cache.ts';
import { LOCAL, cpuOf, localFromState, parseNodes } from './lib/cluster.ts';
import { coresSample, mergeByT, parseHistory, parseRecordedHistory, recordedWindow } from './lib/history.ts';
import type { RecordedBucket, Sample, Tick } from './lib/history.ts';
import { POOL_BASE, machineNumbers, notJsonError } from './lib/headroom.ts';
import type { MachineNumbers } from './lib/headroom.ts';
import { familyOrder, hostSample } from './lib/host.ts';
import type { HostSample } from './lib/host.ts';
import { isActive, jobId } from './lib/job.ts';
import { SparkBuffer } from './lib/spark.ts';
import type { Point } from './lib/spark.ts';

export type { Sample } from './lib/history.ts';

const MAX_SAMPLES = 21600;

export const jobs = shallowReactive(new Map<number, Job>());
export const live = shallowReactive(new Map<number, LiveJob>());
/** Not reactive: read by the canvas loops every frame. */
export const series: Sample[] = [];
/** Not reactive: host census per second, read by the host panel's canvas loops. */
export const hostSeries: HostSample[] = [];
const exact = shallowRef(series);
const recorded = shallowRef<RecordedBucket[]>([]);
/** Not reactive: per-job cores over the last 60 s; readers re-run on `live` changes. */
export const sparks = new SparkBuffer<number>(60_000);
/** Not reactive: per-session cores over the last 60 s; readers re-run on `census` changes. */
export const sessionSparks = new SparkBuffer<string>(60_000);
/** Host census of the newest tick. */
export const census = shallowRef<HostCensus | null>(null);
/** Title of every session a census named, by id: the job tables name a job's session by it, also after the session went quiet. */
export const sessionTitles = shallowReactive(new Map<string, string>());

function noteTitles(ticks: readonly Tick[]): void {
  for (const t of ticks) {
    for (const s of t.census.sessions) if (s.session && s.title && sessionTitles.get(s.session) !== s.title) sessionTitles.set(s.session, s.title);
  }
}

export const store = reactive({
  host: '',
  cores: 0,
  tokens: 0,
  /** Tokens that circulate: the pool less the headroom. */
  limit: 0,
  /** Share of the CPU kept free for interactive work, percent, as the daemon has it. */
  headroom: 0,
  /** Applications in a call while call mode holds the headroom up. */
  callApps: [] as string[],
  /** CPU quota on the daemon unit, percent of one core; null when not capped. */
  cpuQuota: null as number | null,
  /** The machine base's numbers; `headroom_base` is `pool` on a daemon older than 0.16. */
  machine: { ...POOL_BASE } as MachineNumbers,
  /** Admission by share while it is on; null by slots and from a daemon older than shares. */
  shares: null as SharesView | null,
  /** A pending upgrade, as one line from the daemon; empty when none. */
  upgrade: '',
  /** Error of the last headroom change; empty when it went through. */
  headroomError: '',
  /** Count of refused headroom changes; the slider goes back to the daemon's value on each. */
  headroomRefused: 0,
  free: 0,
  jobCores: 0,
  sysCpu: 0,
  toolchains: [] as string[],
  /** Tool families seen in any tick, in stacking order. */
  families: [] as string[],
  connected: false,
  historyLoaded: false,
  /** The first snapshot and the history attempt are done: the page renders its panels at once, so nothing shifts. */
  ready: false,
  error: '',
  clock: Date.now(),
  windowS: 600,
  selected: null as number | null,
  /** Output pane switches to the most recently started running job by itself. */
  followNewest: false,
  /** The pane was asked for by the keyboard and has not taken the focus yet. */
  paneFocus: false,
});

let skew = 0;
let lastTick = -Infinity;

/** Recompute once per tick, history load or zoom; both canvases reuse these samples between frames. */
export const chartWindow = computed(() => {
  const to = store.clock;
  const from = to - store.windowS * 1000;
  const inWindow = (p: { t: number }) => p.t >= from && p.t <= to;
  const samples = exact.value.filter(inWindow);
  const host = hostSeries.filter(inWindow);
  const buckets = recordedWindow(recorded.value, exact.value, from, to);
  let maxHost = 0;
  let maxWaiting = 0;
  for (const b of buckets) {
    maxHost = Math.max(maxHost, b.host_cores);
    maxWaiting = Math.max(maxWaiting, b.waiting);
  }
  for (const p of samples) maxWaiting = Math.max(maxWaiting, p.waiting);
  return { samples, host, buckets, maxHost, maxWaiting };
});

/** Compiler calls per minute as `api/cache-history` answered last; empty from a daemon that has no such path. */
export const cacheMinutes = shallowRef<CacheMinute[]>([]);

/** The compile cache chart: the minutes in the window as shares, the newest minute while it is fresh, and how many minutes are held. */
export const cacheChart = computed(() => {
  const to = store.clock;
  const all = cacheMinutes.value;
  const points = cacheWindow(all, to - store.windowS * 1000, to).map(cachePoint);
  return { points, latest: latestMinute(all, to), recorded: all.length };
});

/** The compiler calls of the last 24 hours as `api/cache-day` answered last; undefined from a daemon that has no such path. */
export const cacheDay = shallowRef<CacheMinute | undefined>(undefined);

/** Nodes as `api/nodes` answered last; null from a daemon without the path (the page then builds the local node from the state). */
export const apiNodes = shallowRef<Node[] | null>(null);
/** Free space the eviction saw last, bytes: the local node's disk from a daemon without `api/nodes`. */
const evictFree = shallowRef(0);
/** Not reactive: busy CPUs per node over the last 60 s, read by the node cards' canvases. */
export const nodeSparks = new SparkBuffer<string>(62_000);

/** Every node the start page shows: the daemon's answer, or the local node from the state alone. */
export const nodes = computed<Node[]>(() => {
  if (apiNodes.value) return apiNodes.value;
  void store.clock;
  return [localFromState({
    host: store.host, cores: store.cores, limit: store.limit, tokens: store.tokens, free: store.free, jobCores: store.jobCores,
    diskFree: evictFree.value,
  }, jobs.values())];
});

let nodesAt = -Infinity;
let nodesMissing = false;

/** Asks `api/nodes` at most once a second; a 404 (older daemon) stops asking until the page loads again. */
async function loadNodes(force = false): Promise<void> {
  if (nodesMissing) return;
  const t = Date.now();
  if (!force && t - nodesAt < 900) return;
  nodesAt = t;
  const res = await fetch('api/nodes');
  if (res.status === 404) {
    nodesMissing = true;
    apiNodes.value = null;
    return;
  }
  if (!res.ok) return;
  const list = parseNodes(await res.json());
  if (!list) return;
  apiNodes.value = list;
  const at = now();
  for (const n of list) nodeSparks.push(n.node_id, at, cpuOf(n));
}

/** Server time estimate, ms. */
export const now = (): number => Date.now() + skew;

const toolchainOf = (id: number): string | undefined => jobs.get(id)?.toolchain;

function replace<T>(arr: T[], items: readonly T[]): void {
  arr.length = 0;
  for (const x of items) arr.push(x);
}

function noteFamilies(ticks: readonly Tick[]): void {
  const seen = new Set(store.families);
  const before = seen.size;
  for (const t of ticks) for (const c of t.census.classes) seen.add(c.class);
  if (seen.size !== before) store.families = familyOrder(seen);
}

async function loadState(): Promise<void> {
  const res = await fetch('api/state');
  if (!res.ok) throw new Error(`api/state: HTTP ${res.status}`);
  const st = (await res.json()) as State;
  skew = Number(st.now_ms) - Date.now();
  store.host = st.host;
  store.cores = st.cores;
  store.tokens = st.tokens;
  store.limit = st.limit;
  store.headroom = st.headroom_percent;
  store.callApps = st.call_apps ?? [];
  store.cpuQuota = st.cpu_quota_percent;
  store.machine = machineNumbers(st);
  store.upgrade = st.upgrade ?? '';
  store.shares = st.shares ?? null;
  store.free = st.free;
  store.toolchains = st.toolchains;
  evictFree.value = st.evict?.free ?? 0;
  store.clock = now();
  for (const j of [...st.done, ...st.active]) jobs.set(jobId(j), j);
}

/** Applies the newest tick: live numbers, and one more sample in every buffer. */
function applyTick(m: Tick): void {
  if (m.t <= lastTick) return;
  lastTick = m.t;
  store.tokens = m.tokens;
  store.limit = m.limit;
  store.headroom = m.headroom_percent;
  store.callApps = m.call_apps ?? [];
  // An older daemon sends no upgrade in its ticks; the line from the snapshot stands.
  if (m.upgrade !== undefined) store.upgrade = m.upgrade;
  store.shares = m.shares ?? null;
  // Likewise the quota: a daemon older than 0.16 sends none in its ticks.
  if (m.cpu_quota_percent !== undefined) store.cpuQuota = m.cpu_quota_percent;
  store.machine = machineNumbers(m);
  store.free = m.free;
  store.jobCores = m.job_cores;
  store.sysCpu = m.sys_cpu;
  census.value = m.census;
  if (!apiNodes.value) nodeSparks.push(LOCAL, m.t, m.job_cores);
  void loadNodes().catch(() => undefined);
  live.clear();
  const ids = new Set<number>();
  for (const l of m.jobs) {
    const id = jobId(l);
    ids.add(id);
    live.set(id, l);
    sparks.push(id, m.t, l.cores);
  }
  sparks.retain(ids);
  const sessions = new Set<string>();
  for (const s of m.census.sessions) {
    sessions.add(s.session);
    sessionSparks.push(s.session, m.t, s.cores);
  }
  sessionSparks.retain(sessions);
  series.push(coresSample(m, toolchainOf, store.cores));
  if (series.length > MAX_SAMPLES) series.splice(0, series.length - MAX_SAMPLES);
  hostSeries.push(hostSample(m));
  if (hostSeries.length > MAX_SAMPLES) hostSeries.splice(0, hostSeries.length - MAX_SAMPLES);
  noteFamilies([m]);
  noteTitles([m]);
  triggerRef(exact);
}

/** Seeds every chart buffer from `api/history`; samples already held win, none is added twice. */
async function loadExactHistory(): Promise<void> {
  const res = await fetch('api/history');
  if (!res.ok) return;
  const ticks = parseHistory(await res.json());
  const newest = ticks[ticks.length - 1];
  if (newest) applyTick(newest);
  replace(series, mergeByT(ticks.map((t) => coresSample(t, toolchainOf, store.cores)), series, MAX_SAMPLES));
  replace(hostSeries, mergeByT(ticks.map(hostSample), hostSeries, MAX_SAMPLES));
  triggerRef(exact);
  const jobPts = new Map<number, Point[]>();
  const sessionPts = new Map<string, Point[]>();
  for (const t of ticks) {
    for (const l of t.jobs) {
      const id = jobId(l);
      const pts = jobPts.get(id) ?? [];
      pts.push({ t: t.t, v: l.cores });
      jobPts.set(id, pts);
    }
    for (const s of t.census.sessions) {
      const pts = sessionPts.get(s.session) ?? [];
      pts.push({ t: t.t, v: s.cores });
      sessionPts.set(s.session, pts);
    }
  }
  noteFamilies(ticks);
  // The local node's CPU line starts with the jobs' cores of the last minute; the answers of `api/nodes` follow it.
  nodeSparks.seed(LOCAL, ticks.map((t) => ({ t: t.t, v: t.job_cores })));
  noteTitles(ticks);
  for (const [id, pts] of jobPts) if (live.has(id)) sparks.seed(id, pts);
  const current = new Set((census.value?.sessions ?? []).map((s) => s.session));
  for (const [key, pts] of sessionPts) if (current.has(key)) sessionSparks.seed(key, pts);
}

async function loadRecordedHistory(): Promise<void> {
  const res = await fetch('api/chart-history');
  if (!res.ok) return;
  const buckets = parseRecordedHistory(await res.json());
  const from = now() - MAX_SAMPLES * 1000;
  recorded.value = mergeByT(recorded.value, buckets).filter((b) => b.t + b.n * 1000 > from);
}

/** A daemon without the path answers 404 and the minutes held stand: the chart then says that it has no data. */
async function loadCacheHistory(): Promise<void> {
  const res = await fetch('api/cache-history');
  if (!res.ok) return;
  cacheMinutes.value = parseCacheHistory(await res.json());
}

/** A daemon older than the page answers 404: the legend then has no 24-hour figure, and that is no error. */
async function loadCacheDay(): Promise<void> {
  const res = await fetch('api/cache-day');
  if (!res.ok) {
    cacheDay.value = undefined;
    return;
  }
  cacheDay.value = parseCacheDay(await res.json());
}

/** The compile cache chart's minutes and its 24-hour sum, asked for side by side. */
async function loadCache(): Promise<void> {
  await Promise.allSettled([loadCacheHistory(), loadCacheDay()]);
}

async function loadHistory(): Promise<void> {
  await Promise.allSettled([loadExactHistory(), loadRecordedHistory(), loadCache()]);
  store.historyLoaded = true;
}

/** Called with each job event, the job as it was before (undefined when new) and as it is now; the start page animates by it. */
export const jobChanges = new Set<(before: Job | undefined, after: Job) => void>();

function apply(m: Event): void {
  if (m.type === 'job') {
    const id = jobId(m.job);
    const before = jobs.get(id);
    for (const f of jobChanges) f(before, m.job);
    jobs.set(id, m.job);
    if (!isActive(m.job.state)) live.delete(id);
    return;
  }
  applyTick(m);
}

function connect(): void {
  const es = new EventSource('api/events');
  let opened = false;
  es.onopen = () => {
    store.connected = true;
    if (opened) {
      void loadState()
        .then(loadHistory)
        .catch(() => undefined);
    }
    opened = true;
  };
  es.onerror = () => {
    store.connected = false;
  };
  es.onmessage = (ev: MessageEvent<string>) => apply(JSON.parse(ev.data) as Event);
}

/** Loads the snapshot (retrying every 2 s), follows the event stream, and seeds the charts from history. */
export async function start(): Promise<void> {
  setInterval(() => {
    store.clock = now();
  }, 1000);
  for (;;) {
    try {
      await loadState();
      store.error = '';
      break;
    } catch (e) {
      store.error = e instanceof Error ? e.message : String(e);
      await new Promise((r) => setTimeout(r, 2000));
    }
  }
  connect();
  await loadHistory().catch(() => undefined);
  await loadNodes(true).catch(() => undefined);
  store.ready = true;
  // The daemon adds a minute of compiler calls each minute, and no event says so.
  setInterval(() => void loadCache(), MINUTE_MS);
}

/**
 * Opens the output pane on a job; closing it (null) also stops following the newest job.
 * `click` is the click that asked for it: one made by the keyboard (Enter or Space on a button,
 * `detail` 0) moves the focus into the pane.
 */
export function select(id: number | null, click?: UIEvent): void {
  store.selected = id;
  store.paneFocus = id !== null && click?.detail === 0;
  if (id === null) store.followNewest = false;
}

let headroomTimer: ReturnType<typeof setTimeout> | undefined;

/** Sends a new headroom to the daemon, 300 ms after the last call. */
export function setHeadroom(percent: number): void {
  clearTimeout(headroomTimer);
  headroomTimer = setTimeout(() => {
    void (async () => {
      try {
        const res = await fetch('api/limits', {
          method: 'PUT',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ headroom_percent: percent }),
        });
        if (!res.ok) throw new Error(`HTTP ${res.status}: ${(await res.text()).trim()}`);
        const older = notJsonError(res.headers.get('content-type'));
        if (older) throw new Error(older);
        const v = (await res.json()) as {
          headroom_percent: number;
          effective_tokens: number;
          cpu_quota_percent: number | null;
          note?: string;
        };
        store.headroom = v.headroom_percent;
        store.limit = v.effective_tokens;
        store.cpuQuota = v.cpu_quota_percent;
        store.headroomError = v.headroom_percent > 0 && v.cpu_quota_percent === null ? `cpu quota not applied: ${v.note ?? ''}` : '';
      } catch (e) {
        store.headroomError = `headroom: ${(e as Error).message}`;
        store.headroomRefused++;
      }
    })();
  }, 300);
}
