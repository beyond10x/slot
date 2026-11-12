import { reactive, shallowReactive } from 'vue';
import type { Event, Job, LiveJob, State } from './generated/types.ts';
import { isActive, jobId } from './lib/job.ts';
import { SparkBuffer } from './lib/spark.ts';

/** One second of the cores chart: host CPU in cores, job cores per toolchain, jobs waiting for slots. */
export interface Sample {
  t: number;
  sys: number;
  by: Record<string, number>;
  waiting: number;
}

const MAX_SAMPLES = 21600;

export const jobs = shallowReactive(new Map<number, Job>());
export const live = shallowReactive(new Map<number, LiveJob>());
/** Not reactive: read by the canvas loops every frame. */
export const series: Sample[] = [];
/** Not reactive: per-job cores over the last 60 s; readers re-run on `live` changes. */
export const sparks = new SparkBuffer(60_000);

export const store = reactive({
  host: '',
  cores: 0,
  tokens: 0,
  free: 0,
  jobCores: 0,
  sysCpu: 0,
  toolchains: [] as string[],
  connected: false,
  error: '',
  clock: Date.now(),
  windowS: 600,
  selected: null as number | null,
  /** Output pane switches to the most recently started running job by itself. */
  followNewest: false,
});

let skew = 0;

/** Server time estimate, ms. */
export const now = (): number => Date.now() + skew;

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

function apply(m: Event): void {
  if (m.type === 'job') {
    const id = jobId(m.job);
    jobs.set(id, m.job);
    if (!isActive(m.job.state)) live.delete(id);
    return;
  }
  store.tokens = m.tokens;
  store.free = m.free;
  store.jobCores = m.job_cores;
  store.sysCpu = m.sys_cpu;
  const by: Record<string, number> = {};
  live.clear();
  const t = Number(m.t);
  const ids = new Set<number>();
  for (const l of m.jobs) {
    const id = jobId(l);
    ids.add(id);
    live.set(id, l);
    sparks.push(id, t, l.cores);
    const tc = jobs.get(id)?.toolchain ?? 'other';
    by[tc] = (by[tc] ?? 0) + l.cores;
  }
  sparks.retain(ids);
  series.push({ t, sys: m.sys_cpu * store.cores, by, waiting: m.waiting });
  if (series.length > MAX_SAMPLES) series.splice(0, series.length - MAX_SAMPLES);
}

function connect(): void {
  const es = new EventSource('api/events');
  let opened = false;
  es.onopen = () => {
    store.connected = true;
    if (opened) void loadState().catch(() => undefined);
    opened = true;
  };
  es.onerror = () => {
    store.connected = false;
  };
  es.onmessage = (ev: MessageEvent<string>) => apply(JSON.parse(ev.data) as Event);
}

/** Loads the snapshot (retrying every 2 s), then follows the event stream. */
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
}

/** Opens the output pane on a job; closing it (null) also stops following the newest job. */
export function select(id: number | null): void {
  store.selected = id;
  if (id === null) store.followNewest = false;
}
