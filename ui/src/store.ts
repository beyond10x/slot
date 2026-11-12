import { reactive, shallowReactive, shallowRef } from 'vue';
import type { Event, HostCensus, Job, LiveJob, State } from './generated/types.ts';
import { coresSample, mergeByT, parseHistory } from './lib/history.ts';
import type { Sample, Tick } from './lib/history.ts';
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
/** Not reactive: per-job cores over the last 60 s; readers re-run on `live` changes. */
export const sparks = new SparkBuffer<number>(60_000);
/** Not reactive: per-session cores over the last 60 s; readers re-run on `census` changes. */
export const sessionSparks = new SparkBuffer<string>(60_000);
/** Host census of the newest tick. */
export const census = shallowRef<HostCensus | null>(null);

export const store = reactive({
  host: '',
  cores: 0,
  tokens: 0,
  free: 0,
  jobCores: 0,
  sysCpu: 0,
  toolchains: [] as string[],
  /** Tool families seen in any tick, in stacking order. */
  families: [] as string[],
  connected: false,
  historyLoaded: false,
  error: '',
  clock: Date.now(),
  windowS: 600,
  selected: null as number | null,
  /** Output pane switches to the most recently started running job by itself. */
  followNewest: false,
});

let skew = 0;
let lastTick = -Infinity;

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
  store.free = st.free;
  store.toolchains = st.toolchains;
  store.clock = now();
  for (const j of [...st.done, ...st.active]) jobs.set(jobId(j), j);
}

/** Applies the newest tick: live numbers, and one more sample in every buffer. */
function applyTick(m: Tick): void {
  if (m.t <= lastTick) return;
  lastTick = m.t;
  store.tokens = m.tokens;
  store.free = m.free;
  store.jobCores = m.job_cores;
  store.sysCpu = m.sys_cpu;
  census.value = m.census;
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
}

/** Seeds every chart buffer from `api/history`; samples already held win, none is added twice. */
async function loadHistory(): Promise<void> {
  const res = await fetch('api/history');
  if (!res.ok) return;
  const ticks = parseHistory(await res.json());
  replace(series, mergeByT(ticks.map((t) => coresSample(t, toolchainOf, store.cores)), series, MAX_SAMPLES));
  replace(hostSeries, mergeByT(ticks.map(hostSample), hostSeries, MAX_SAMPLES));
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
  const newest = ticks[ticks.length - 1];
  // Without a live tick yet, the newest history tick is the current state.
  if (newest && newest.t > lastTick) applyTick(newest);
  for (const [id, pts] of jobPts) if (live.has(id)) sparks.seed(id, pts);
  const current = new Set((census.value?.sessions ?? []).map((s) => s.session));
  for (const [key, pts] of sessionPts) if (current.has(key)) sessionSparks.seed(key, pts);
  store.historyLoaded = true;
}

function apply(m: Event): void {
  if (m.type === 'job') {
    const id = jobId(m.job);
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
}

/** Opens the output pane on a job; closing it (null) also stops following the newest job. */
export function select(id: number | null): void {
  store.selected = id;
  if (id === null) store.followNewest = false;
}
