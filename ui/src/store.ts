import { reactive, shallowReactive, shallowRef } from 'vue';
import type { Event, HostCensus, Job, LiveJob, State } from './generated/types.ts';
import { coresSample, mergeByT, parseHistory } from './lib/history.ts';
import type { Sample, Tick } from './lib/history.ts';
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
  store.limit = st.limit;
  store.headroom = st.headroom_percent;
  store.callApps = st.call_apps ?? [];
  store.cpuQuota = st.cpu_quota_percent;
  store.machine = machineNumbers(st);
  store.upgrade = st.upgrade ?? '';
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
  store.limit = m.limit;
  store.headroom = m.headroom_percent;
  store.callApps = m.call_apps ?? [];
  // An older daemon sends no upgrade in its ticks; the line from the snapshot stands.
  if (m.upgrade !== undefined) store.upgrade = m.upgrade;
  // Likewise the quota: a daemon older than 0.16 sends none in its ticks.
  if (m.cpu_quota_percent !== undefined) store.cpuQuota = m.cpu_quota_percent;
  store.machine = machineNumbers(m);
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
  store.ready = true;
}

/** Opens the output pane on a job; closing it (null) also stops following the newest job. */
export function select(id: number | null): void {
  store.selected = id;
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
