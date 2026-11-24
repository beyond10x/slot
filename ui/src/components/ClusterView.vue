<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, reactive, ref, useTemplateRef, watch, watchEffect } from 'vue';
import type { Job, Node } from '../generated/types.ts';
import { QUEUE_COLOR, tcColor } from '../lib/colors.ts';
import { LOCAL, cacheShare, checkoutOf, cloudCost, cpuTotals, jobList, laneColor, nodeJobs, nodeOf } from '../lib/cluster.ts';
import { fmtDur } from '../lib/format.ts';
import { cmdText, isActive, jobId, runS, waitS } from '../lib/job.ts';
import { servedQueue, waitReasonText, waitStats } from '../lib/queue.ts';
import { cacheDay, chartWindow, jobChanges, jobs, live, nodes, select, store } from '../store.ts';
import NodeCard from './NodeCard.vue';

const props = defineProps<{ theme: 'dark' | 'light' }>();
const emit = defineEmits<{ theme: ['dark' | 'light'] }>();
const dark = computed(() => props.theme === 'dark');

const reduce = ref(matchMedia('(prefers-reduced-motion: reduce)').matches);

/** Up to three nodes in the diagram; a bay without one says where remote nodes join. */
const shown = computed(() => nodes.value.filter((n) => n.state !== 'Gone').slice(0, 3));
const ghost = computed(() => shown.value.length < 3 && !shown.value.some((n) => n.kind === 'Remote'));
const nameOf = (id: string): string => (id === LOCAL ? store.host || LOCAL : id);

const filter = ref<string | null>(null);
function toggle(id: string): void {
  filter.value = filter.value === id ? null : id;
}
function clearFilter(): void {
  const id = filter.value;
  filter.value = null;
  if (id) void nextTick(() => document.querySelector<HTMLElement>(`[data-node="${CSS.escape(id)}"]`)?.focus());
}
function onKey(e: KeyboardEvent): void {
  if (e.key === 'Escape' && filter.value) {
    e.preventDefault();
    filter.value = null;
  }
}
// A node that left takes its filter with it.
watch(nodes, (list) => {
  if (filter.value && !list.some((n) => n.node_id === filter.value)) filter.value = null;
});

const allJobs = computed(() => [...jobs.values()]);
const running = computed(() => allJobs.value.filter((j) => j.state === 'running'));
const runningOn = (id: string): Job[] => running.value.filter((j) => nodeOf(j) === id);
const queue = computed(() => servedQueue(jobs.values()));
/** Per node: its checkouts with a job now and its finished and failed jobs, from the jobs of `api/state`. */
const perNode = computed(() => new Map(shown.value.map((n) => [n.node_id, nodeJobs(n.node_id, allJobs.value)])));
const EMPTY_JOBS = { checkouts: [], done: 0, failed: 0 };

/** Headline numbers. */
const kpi = computed(() => {
  const w = waitStats(jobs.values(), store.clock);
  const cpu = cpuTotals(nodes.value);
  return {
    running: running.value.length, queued: queue.value.length, waitP90: w.p90, cpu,
    cache: cacheShare(cacheDay.value), cost: cloudCost(nodes.value, store.clock),
  };
});
/** Numbers ease to their new value; with reduced motion they jump. */
const shownKpi = reactive({ running: 0, queued: 0, wait: 0, cpu: 0, cache: 0, cost: 0 });
const targets = computed(() => ({
  running: kpi.value.running, queued: kpi.value.queued, wait: kpi.value.waitP90 ?? 0, cpu: kpi.value.cpu.used,
  cache: kpi.value.cache ?? 0, cost: kpi.value.cost?.total ?? 0,
}));
let tweenFrame = 0;
function tween(): void {
  cancelAnimationFrame(tweenFrame);
  const t = targets.value;
  if (reduce.value) {
    Object.assign(shownKpi, t);
    return;
  }
  let last = performance.now();
  const step = (now: number): void => {
    const a = 1 - Math.exp(-(now - last) / 220);
    last = now;
    let moving = false;
    for (const k of Object.keys(t) as Array<keyof typeof t>) {
      const d = t[k] - shownKpi[k];
      if (Math.abs(d) < 0.005) shownKpi[k] = t[k];
      else {
        shownKpi[k] += d * a;
        moving = true;
      }
    }
    if (moving) tweenFrame = requestAnimationFrame(step);
  };
  tweenFrame = requestAnimationFrame(step);
}
watch(targets, tween, { immediate: true });

const connection = computed(() => (store.connected ? 'connected' : 'not connected'));

/** The job list: newest first, as many rows as fit. */
const RH = 28;
const rowsBox = useTemplateRef<HTMLElement>('rowsBox');
const rowCap = ref(8);
const list = computed(() => jobList(jobs.values(), filter.value, rowCap.value));
const jobSum = computed(() => {
  const f = filter.value;
  const on = (j: Job): boolean => f === null || nodeOf(j) === f;
  let run = 0;
  let fin = 0;
  let failed = 0;
  for (const j of jobs.values()) {
    if (!on(j)) continue;
    if (j.state === 'running') run++;
    else if (!isActive(j.state) && Number(j.ended_ms) > store.clock - 300_000) {
      fin++;
      if (j.state === 'failed') failed++;
    }
  }
  return { run, fin, failed };
});
const cpuCell = (j: Job): string => {
  if (j.state === 'queued') return `×${j.tokens}`;
  if (j.state === 'running') {
    const l = live.get(jobId(j));
    return `${(l?.cores ?? 0).toFixed(1)} / ${j.tokens}`;
  }
  const r = runS(j, store.clock);
  return r && r > 0 ? `${(j.cpu_s / r).toFixed(1)} avg` : '–';
};
const stateWord = (j: Job): string => j.state;
const toolNames = computed(() => [...new Set(allJobs.value.map((j) => j.toolchain))].slice(0, 5));

function openOutput(j: Job, e: MouseEvent): void {
  select(jobId(j), e);
  location.hash = '#jobs';
}

/** Queue chips: as many as fit, oldest place first. */
const QH = 37;
const qBox = useTemplateRef<HTMLElement>('qBox');
const qCap = ref(6);
const chips = computed(() => queue.value.slice(0, queue.value.length > qCap.value ? Math.max(0, qCap.value - 1) : qCap.value));
const oldestWait = computed(() => (queue.value.length ? Math.max(...queue.value.map((j) => waitS(j, store.clock))) : 0));
const queueMeta = (j: Job): string => {
  const reason = j.wait_reason ? waitReasonText(j) : 'waits for a slot';
  return `${j.tokens} ${j.tokens === 1 ? 'slot' : 'slots'} · ${j.priority !== 'normal' ? j.priority + ' · ' : ''}${reason}`;
};

let ro: ResizeObserver | null = null;
function measure(): void {
  if (rowsBox.value) rowCap.value = Math.max(1, Math.floor(rowsBox.value.clientHeight / RH));
  if (qBox.value) qCap.value = Math.max(1, Math.floor((qBox.value.clientHeight - 20) / QH) + 1);
  resizeFlow();
}

/* ---- the canvas over the diagram: a lane per checkout from the queue to its node, jobs moving along them ---- */
const diagram = useTemplateRef<HTMLElement>('diagram');
const flow = useTemplateRef<HTMLCanvasElement>('flow');
let W = 0;
let H = 0;
let DPR = 1;
function resizeFlow(): void {
  const d = diagram.value;
  const cv = flow.value;
  if (!d || !cv) return;
  DPR = window.devicePixelRatio || 1;
  W = d.clientWidth;
  H = d.clientHeight;
  cv.width = Math.round(W * DPR);
  cv.height = Math.round(H * DPR);
  drawFlow(performance.now());
}
interface Box { x: number; y: number; w: number; h: number }
function rel(el: Element | null): Box | null {
  const d = diagram.value;
  if (!el || !d) return null;
  const a = el.getBoundingClientRect();
  const b = d.getBoundingClientRect();
  return { x: a.left - b.left, y: a.top - b.top, w: a.width, h: a.height };
}
interface Lane { name: string; node: string; x0: number; y0: number; x1: number; y1: number }
/** The lanes of the checkouts that have a job on a node now. */
function lanes(): Lane[] {
  const d = diagram.value;
  const ep = rel(d?.querySelector('.endpoint') ?? null);
  if (!d || !ep) return [];
  const all = [...new Set(allJobs.value.filter((j) => isActive(j.state)).map(checkoutOf))].sort();
  const out: Lane[] = [];
  for (const n of shown.value) {
    const g = rel(d.querySelector(`[data-node="${CSS.escape(n.node_id)}"] .slots`));
    if (!g) continue;
    const here = perNode.value.get(n.node_id)?.checkouts ?? [];
    here.forEach((w, k) => {
      const i = Math.max(0, all.indexOf(w));
      out.push({
        name: w, node: n.node_id,
        x0: ep.x + ep.w, y0: ep.y + ep.h * 0.5 + (i - (all.length - 1) / 2) * 9,
        x1: g.x - 6, y1: g.y + g.h / 2 + (k - (here.length - 1) / 2) * Math.min(9, g.h / Math.max(1, here.length)),
      });
    });
  }
  return out;
}
const bez = (p0: number, p1: number, p2: number, p3: number, t: number): number => {
  const u = 1 - t;
  return u * u * u * p0 + 3 * u * u * t * p1 + 3 * u * t * t * p2 + t * t * t * p3;
};
function lanePoints(L: Lane, steps = 24): Array<[number, number]> {
  const mx = (L.x0 + L.x1) / 2;
  const pts: Array<[number, number]> = [];
  for (let s = 0; s <= steps; s++) pts.push([bez(L.x0, mx, mx, L.x1, s / steps), bez(L.y0, L.y0, L.y1, L.y1, s / steps)]);
  return pts;
}
function atLen(pts: Array<[number, number]>, d: number): [number, number] {
  for (let i = 1; i < pts.length; i++) {
    const s = Math.hypot(pts[i][0] - pts[i - 1][0], pts[i][1] - pts[i - 1][1]);
    if (d <= s) {
      const t = s ? d / s : 0;
      return [pts[i - 1][0] + (pts[i][0] - pts[i - 1][0]) * t, pts[i - 1][1] + (pts[i][1] - pts[i - 1][1]) * t];
    }
    d -= s;
  }
  return pts[pts.length - 1];
}
const polyLen = (pts: Array<[number, number]>): number => {
  let l = 0;
  for (let i = 1; i < pts.length; i++) l += Math.hypot(pts[i][0] - pts[i - 1][0], pts[i][1] - pts[i - 1][1]);
  return l;
};
const ease = (t: number): number => (t < 0.5 ? 2 * t * t : 1 - Math.pow(-2 * t + 2, 2) / 2);
interface Particle { pts: Array<[number, number]>; len: number; t0: number; dur: number; color: string; r: number; node: string }
interface Ripple { x: number; y: number; t0: number; dur: number; color: string; max: number; node: string }
const particles: Particle[] = [];
const ripples: Ripple[] = [];
let dash = 0;
let lastFrame = 0;
let frame = 0;

function cssVar(name: string): string {
  return diagram.value ? getComputedStyle(diagram.value).getPropertyValue(name).trim() : '';
}

/** A job that starts leaves its queue chip and moves along its checkout's lane to its node's cells. */
function onJob(before: Job | undefined, after: Job): void {
  if (reduce.value || !diagram.value) return;
  const id = jobId(after);
  if (after.state === 'running' && before?.state !== 'running') {
    const start = rel(diagram.value.querySelector(`[data-chip="${id}"]`)) ?? rel(diagram.value.querySelector('.qlist'));
    if (!start) return;
    const from: [number, number] = [start.x + start.w - 6, start.y + Math.min(start.h / 2, 17)];
    void nextTick(() => {
      const L = lanes().find((l) => l.name === checkoutOf(after) && l.node === nodeOf(after));
      const cell = rel(diagram.value?.querySelector(`[data-cell-job="${id}"]`) ?? null);
      if (!L || !cell) return;
      const pts: Array<[number, number]> = [from, ...lanePoints(L), [cell.x + cell.w / 2, cell.y + cell.h / 2]];
      const len = polyLen(pts);
      particles.push({ pts, len, t0: performance.now(), dur: Math.min(1500, Math.max(650, len / 0.6)), color: tcColor(after.toolchain),
        r: 2.6 + Math.sqrt(after.tokens) * 1.2, node: nodeOf(after) });
      wake();
    });
  } else if (before?.state === 'running' && (after.state === 'ok' || after.state === 'failed')) {
    const c = rel(diagram.value.querySelector(`[data-cell-job="${id}"]`));
    if (!c) return;
    const failed = after.state === 'failed';
    ripples.push({ x: c.x + c.w / 2, y: c.y + c.h / 2, t0: performance.now(), dur: failed ? 1100 : 700, color: cssVar(failed ? '--bad' : '--ok'),
      max: failed ? 26 : 10 + after.tokens * 1.5, node: nodeOf(after) });
    wake();
  }
}

function drawFlow(now: number): void {
  const cv = flow.value;
  const g = cv?.getContext('2d');
  if (!cv || !g) return;
  const dt = lastFrame ? Math.min(100, now - lastFrame) : 16;
  lastFrame = now;
  g.setTransform(DPR, 0, 0, DPR, 0, 0);
  g.clearRect(0, 0, W, H);
  const cpuBy = new Map<string, number>();
  for (const j of running.value) cpuBy.set(checkoutOf(j), (cpuBy.get(checkoutOf(j)) ?? 0) + (live.get(jobId(j))?.cores ?? 0));
  if (!reduce.value) dash += dt;
  for (const L of lanes()) {
    const cpu = cpuBy.get(L.name) ?? 0;
    const dim = filter.value && filter.value !== L.node ? 0.12 : 1;
    const mx = (L.x0 + L.x1) / 2;
    g.beginPath();
    g.moveTo(L.x0, L.y0);
    g.bezierCurveTo(mx, L.y0, mx, L.y1, L.x1, L.y1);
    g.lineWidth = 1.2 + Math.min(2.2, cpu / 6);
    g.strokeStyle = laneColor(L.name, dark.value, (0.18 + Math.min(0.4, cpu / 30)) * dim);
    g.setLineDash([]);
    g.stroke();
    if (cpu > 0.2) {
      g.setLineDash([3, 9]);
      g.lineDashOffset = -(dash * (0.004 + cpu / 900)) % 1200;
      g.strokeStyle = laneColor(L.name, dark.value, 0.75 * dim);
      g.lineWidth = 1.6;
      g.stroke();
      g.setLineDash([]);
    }
    g.beginPath();
    g.arc(L.x1, L.y1, 2.5, 0, Math.PI * 2);
    g.fillStyle = laneColor(L.name, dark.value, 0.9 * dim);
    g.fill();
  }
  for (let i = particles.length - 1; i >= 0; i--) {
    const p = particles[i];
    const t = (now - p.t0) / p.dur;
    if (t >= 1) {
      particles.splice(i, 1);
      continue;
    }
    const dim = filter.value && filter.value !== p.node ? 0.12 : 1;
    for (let k = 6; k >= 0; k--) {
      const [x, y] = atLen(p.pts, ease(Math.max(0, t - k * 0.018)) * p.len);
      g.beginPath();
      g.arc(x, y, k === 0 ? p.r : p.r * (1 - k / 8), 0, Math.PI * 2);
      g.globalAlpha = (k === 0 ? 1 : 0.28 * (1 - k / 7)) * dim;
      g.fillStyle = p.color;
      if (k === 0 && dark.value) {
        g.shadowColor = p.color;
        g.shadowBlur = 10;
      }
      g.fill();
      g.shadowBlur = 0;
    }
    g.globalAlpha = 1;
  }
  for (let i = ripples.length - 1; i >= 0; i--) {
    const r = ripples[i];
    const t = (now - r.t0) / r.dur;
    if (t >= 1) {
      ripples.splice(i, 1);
      continue;
    }
    const dim = filter.value && filter.value !== r.node ? 0.12 : 1;
    g.beginPath();
    g.arc(r.x, r.y, 4 + r.max * ease(t), 0, Math.PI * 2);
    g.strokeStyle = r.color;
    g.globalAlpha = (1 - t) * 0.9 * dim;
    g.lineWidth = 2;
    g.stroke();
    g.globalAlpha = 1;
  }
}
/** With motion the canvas runs every frame while anything moves (a particle, a ring, a busy lane); without, it is drawn on each change. */
function loop(now: number): void {
  frame = 0;
  drawFlow(now);
  const busy = particles.length > 0 || ripples.length > 0 || running.value.length > 0;
  if (!reduce.value && busy) frame = requestAnimationFrame(loop);
}
function wake(): void {
  if (!frame) frame = requestAnimationFrame(loop);
}
watch([() => store.clock, nodes, filter, dark, () => jobs.size], () => void nextTick(wake));

/* ---- the queue's 60 s line ---- */
const qspark = useTemplateRef<HTMLCanvasElement>('qspark');
function drawQueueSpark(): void {
  const cv = qspark.value;
  if (!cv) return;
  const w = cv.clientWidth;
  const h = cv.clientHeight;
  if (!w || !h) return;
  const dpr = window.devicePixelRatio || 1;
  if (cv.width !== Math.round(w * dpr)) {
    cv.width = Math.round(w * dpr);
    cv.height = Math.round(h * dpr);
  }
  const g = cv.getContext('2d');
  if (!g) return;
  g.setTransform(dpr, 0, 0, dpr, 0, 0);
  g.clearRect(0, 0, w, h);
  const t1 = store.clock;
  const pts = chartWindow.value.samples.filter((p) => p.t >= t1 - 60_000);
  if (pts.length < 2) return;
  const max = Math.max(8, ...pts.map((p) => p.waiting));
  g.beginPath();
  pts.forEach((p, i) => {
    const x = Math.max(0, ((p.t - (t1 - 60_000)) / 60_000) * w);
    const y = h - 2 - (h - 4) * Math.min(1, p.waiting / max);
    if (i) g.lineTo(x, y);
    else g.moveTo(x, y);
  });
  g.strokeStyle = cssVar('--queue') || QUEUE_COLOR;
  g.lineWidth = 1.5;
  g.stroke();
}
watch(() => store.clock, () => void nextTick(drawQueueSpark));

const mq = matchMedia('(prefers-reduced-motion: reduce)');
const onMotion = (e: MediaQueryListEvent): void => {
  reduce.value = e.matches;
};
onMounted(() => {
  document.addEventListener('keydown', onKey);
  mq.addEventListener('change', onMotion);
  jobChanges.add(onJob);
  ro = new ResizeObserver(measure);
  if (diagram.value) ro.observe(diagram.value);
  if (rowsBox.value) ro.observe(rowsBox.value);
  measure();
  drawQueueSpark();
});
onUnmounted(() => {
  document.removeEventListener('keydown', onKey);
  mq.removeEventListener('change', onMotion);
  jobChanges.delete(onJob);
  ro?.disconnect();
  cancelAnimationFrame(frame);
  cancelAnimationFrame(tweenFrame);
});

watchEffect(() => {
  if (!store.host) return;
  document.title = `${kpi.value.running} running, ${kpi.value.queued} queued · slot cluster ${store.host}`;
});

const fmtWait = (s: number): string => (s < 60 ? `${s.toFixed(1)}` : fmtDur(s));
const nodeTitle = (n: Node): string => nameOf(n.node_id);
</script>

<template>
  <div class="cluster" data-ux-view="cluster" :data-theme="theme">
    <header class="top">
      <h1 class="brand">
        <span class="name">slot <i>cluster</i></span>
        <span class="sub"><span class="conn" :class="{ ok: store.connected }" data-ux-region="connection" :title="connection"><i aria-hidden="true"></i><span class="sr">{{ connection }}, </span></span>endpoint {{ store.host }}<template v-if="store.cores"> · {{ store.cores }} cores</template></span>
      </h1>
      <dl class="kpis">
        <div class="kpi" data-ux-region="running"><dt>running</dt><dd>{{ Math.round(shownKpi.running) }}<small>jobs</small></dd></div>
        <div class="kpi" data-ux-region="queue"><dt>queued</dt><dd>{{ Math.round(shownKpi.queued) }}<small>jobs</small></dd></div>
        <div class="kpi" title="9 of 10 jobs that finished in the last 60 min waited at most this long for a slot"><dt>wait p90</dt>
          <dd>{{ kpi.waitP90 === null ? '–' : fmtWait(shownKpi.wait) }}<small v-if="kpi.waitP90 !== null && shownKpi.wait < 60">s</small></dd></div>
        <div class="kpi cpu" data-ux-region="cpus"><dt>CPUs in use / available</dt>
          <dd>{{ shownKpi.cpu.toFixed(1) }}<small>/ {{ kpi.cpu.available }}</small>
            <span class="meter" aria-hidden="true"><i :style="{ width: (100 * Math.min(1, kpi.cpu.used / Math.max(1, kpi.cpu.available))).toFixed(1) + '%' }"></i></span></dd></div>
        <div class="kpi" title="share of the compiler calls of the last 24 hours that the compile cache answered"><dt>cache hits, 24 h</dt>
          <dd><template v-if="kpi.cache !== null">{{ (shownKpi.cache * 100).toFixed(1) }}<small>%</small></template><template v-else>–</template></dd></div>
        <div v-if="kpi.cost" class="kpi cost"><dt>cloud cost so far</dt>
          <dd>{{ shownKpi.cost.toFixed(2) }}<small>{{ kpi.cost.perHour.toFixed(2) }}/h</small></dd></div>
      </dl>
      <nav class="right" aria-label="views">
        <a class="btn" href="#jobs" title="job tables, charts, headroom, pool share and every job's output">Jobs and charts</a>
        <a class="btn" href="#/jobs" title="every job with a search and a state filter, and each job's detail">Job list</a>
        <button class="btn" type="button" :aria-pressed="theme === 'light'" @click="emit('theme', dark ? 'light' : 'dark')">{{ dark ? 'Light theme' : 'Dark theme' }}</button>
      </nav>
    </header>

    <main>
      <section ref="diagram" class="diagram" aria-label="Cluster">
        <div class="endpoint">
          <div class="ehead"><b>Endpoint</b><span>queue · scheduler · job log</span></div>
          <div class="esub">places jobs on <b>{{ shown.length }}</b> {{ shown.length === 1 ? 'node' : 'nodes' }}</div>
          <div ref="qBox" class="qlist" role="list" aria-label="Queue">
            <TransitionGroup name="chip">
              <div v-for="(j, i) in chips" :key="jobId(j)" class="qchip" role="listitem" :data-chip="jobId(j)" :class="{ dim: filter && nodeOf(j) !== filter }"
                :style="{ transform: `translateY(${i * QH}px)` }" :title="`#${jobId(j)} ${cmdText(j)} · ${checkoutOf(j)} · ${queueMeta(j)}`">
                <span class="wt" :style="{ background: laneColor(checkoutOf(j), dark) }"></span>
                <span class="td" :style="{ background: tcColor(j.toolchain) }"></span>
                <span class="name">{{ checkoutOf(j) }}</span><span class="age">{{ fmtDur(waitS(j, store.clock)) }}</span>
                <span class="meta">{{ queueMeta(j) }}</span>
              </div>
            </TransitionGroup>
            <div v-if="queue.length > chips.length" class="qmore">+{{ queue.length - chips.length }} more queued</div>
            <div v-if="!queue.length" class="qempty">Queue empty: every job has a slot.</div>
          </div>
          <div class="efoot"><span>queue, 60 s<br><b>{{ queue.length }}</b> queued · oldest <b>{{ fmtDur(oldestWait) }}</b></span><canvas ref="qspark" aria-hidden="true"></canvas></div>
        </div>
        <div aria-hidden="true"></div>
        <div class="nodes">
          <div v-for="n in shown" :key="n.node_id" class="bay">
            <NodeCard :node="n" :name="nodeTitle(n)" :running="runningOn(n.node_id)" :jobs="perNode.get(n.node_id) ?? EMPTY_JOBS" :pressed="filter === n.node_id"
              :dim="!!filter && filter !== n.node_id" :dark="dark"
              @toggle="toggle(n.node_id)" />
          </div>
          <div v-if="ghost" class="bay"><div class="ghost">No remote node. Remote build machines join here when the pool adds them.</div></div>
        </div>
        <canvas ref="flow" class="flow" aria-hidden="true"></canvas>
      </section>

      <section class="jobs" aria-label="Jobs">
        <div class="jhead">
          <h2><span class="name">{{ filter ? `Jobs on ${nameOf(filter)}` : 'Jobs' }}</span> newest first</h2>
          <span class="sum"><b>{{ jobSum.run }}</b> running · <b>{{ jobSum.fin }}</b> finished in 5 min, <b class="bad">{{ jobSum.failed }}</b> failed</span>
          <span v-if="filter" class="fchip">node <b>{{ nameOf(filter) }}</b><kbd>Esc</kbd>
            <button type="button" aria-label="Clear the node filter" @click="clearFilter">✕</button></span>
          <span class="legend" aria-hidden="true"><span v-for="t in toolNames" :key="t" :style="{ '--c': tcColor(t) }">{{ t }}</span></span>
        </div>
        <div class="jtable" role="table" aria-label="Jobs, newest first">
          <div class="jcols" role="row">
            <span role="columnheader">job</span><span role="columnheader">state</span><span role="columnheader">tool</span><span role="columnheader">checkout</span>
            <span role="columnheader">node</span><span role="columnheader">command</span><span role="columnheader" class="r">wait</span>
            <span role="columnheader" class="r">run</span><span role="columnheader" class="r">CPU</span>
          </div>
          <div ref="rowsBox" class="jrows" role="rowgroup">
            <TransitionGroup name="row">
              <div v-for="(j, i) in list" :key="jobId(j)" class="jrow" role="row" :style="{ transform: `translateY(${i * RH}px)` }">
                <span role="cell" class="id"><button type="button" class="jobid" :aria-label="`output of job ${jobId(j)}`" :title="`open the output of job ${jobId(j)} in the job view`"
                  @click="openOutput(j, $event)">{{ jobId(j) }}</button></span>
                <span role="cell"><span class="st" :class="j.state"><i></i>{{ stateWord(j) }}</span></span>
                <span role="cell" class="tool" :title="j.toolchain"><i :style="{ background: tcColor(j.toolchain) }"></i>{{ j.toolchain }}</span>
                <span role="cell" class="wt" :title="checkoutOf(j)"><i :style="{ background: laneColor(checkoutOf(j), dark) }"></i>{{ checkoutOf(j) }}</span>
                <span role="cell" class="nd" :title="nameOf(nodeOf(j))">{{ j.state === 'queued' ? '→ ' : '' }}{{ nameOf(nodeOf(j)) }}</span>
                <span role="cell" class="cmd" :title="cmdText(j)">{{ cmdText(j) }}</span>
                <span role="cell" class="r num">{{ fmtDur(waitS(j, store.clock)) }}</span>
                <span role="cell" class="r num">{{ runS(j, store.clock) === null ? '' : fmtDur(runS(j, store.clock) ?? 0) }}</span>
                <span role="cell" class="r num">{{ cpuCell(j) }}</span>
              </div>
            </TransitionGroup>
            <div v-if="!list.length" class="jempty">{{ filter ? 'No job on this node yet.' : 'No job yet.' }}</div>
          </div>
        </div>
      </section>
    </main>
  </div>
</template>

<style scoped>
.cluster {
  --fg: #1a1a1a; --mute: #5c5c5c; --line: #d4d4d4; --rule: #e9e9e9; --bg: #fff; --panel: #f6f6f6; --panel2: #eef0f3;
  --hover: #f2f4f7; --ink: #3d3d3d; --accent: #1f5fbf; --bad: #b00000; --ok: #1e7e34; --warn: #a94300;
  --ctl: #8a8a8a; --cell: #e3e5e8; --track: #e3e5e8; --queue: #a8457f;
  display: grid; grid-template-rows: 56px minmax(0, 1fr); height: 100vh; min-height: 620px; color: var(--fg); background: var(--bg);
  transition: background-color .3s, color .3s;
}
.cluster[data-theme='dark'] {
  --fg: #e6e9ed; --mute: #9aa5b1; --line: #2c343e; --rule: #20262e; --bg: #0e1115; --panel: #151a20; --panel2: #1b2129;
  --hover: #1c232c; --ink: #c9d1d9; --accent: #6ea8fe; --bad: #ff7b72; --ok: #56d364; --warn: #f0a04b;
  --ctl: #5b6673; --cell: #222a33; --track: #252d37; --queue: #CC79A7;
}
main { display: grid; grid-template-rows: auto minmax(0, 1fr); min-height: 0; }
.btn { min-height: 26px; padding: 0 9px; border: 1px solid var(--ctl); border-radius: 5px; background: transparent; color: var(--fg); cursor: pointer;
  font-size: 13px; line-height: 24px; white-space: nowrap; text-decoration: none; display: inline-flex; align-items: center; }
.btn:hover { background: var(--hover); }
.btn[aria-pressed='true'], .btn[aria-pressed='true']:hover { background: transparent; border-color: var(--ctl); color: var(--fg); }

header.top { display: flex; align-items: center; gap: 18px; padding: 0 14px; border-bottom: 1px solid var(--line); min-width: 0; }
.brand { display: flex; flex-direction: column; line-height: 18px; flex: none; margin: 0; font-size: inherit; font-weight: inherit; }
.brand .name { font-size: 17px; font-weight: 700; letter-spacing: .2px; }
.brand .name i { font-style: normal; font-weight: 400; color: var(--mute); }
.brand .sub { font-size: 12px; color: var(--mute); white-space: nowrap; }
.conn i { display: inline-block; width: 8px; height: 8px; border-radius: 50%; background: var(--bad); margin-right: 5px; vertical-align: 0; }
.conn.ok i { background: var(--ok); animation: beat 2s infinite; }
@keyframes beat { 0% { box-shadow: 0 0 0 0 color-mix(in srgb, var(--ok) 55%, transparent); } 70%, 100% { box-shadow: 0 0 0 7px transparent; } }
.kpis { display: flex; gap: 6px; margin: 0; flex: 1; min-width: 0; justify-content: center; }
.kpi { flex: none; width: 96px; padding: 3px 10px; border-left: 1px solid var(--line); }
.kpi:first-child { border-left: 0; }
.kpi.cpu { width: 172px; }
.kpi.cost { width: 150px; }
.kpi:nth-child(5) { width: 120px; }
.kpi dt { font-size: 12px; line-height: 15px; color: var(--mute); white-space: nowrap; }
.kpi dd { margin: 0; font-size: 17px; line-height: 23px; font-weight: 700; font-variant-numeric: tabular-nums; white-space: nowrap; }
.kpi dd small { font-size: 12px; font-weight: 400; color: var(--mute); margin-left: 3px; }
.kpi .meter { display: block; height: 3px; margin-top: -1px; border-radius: 2px; background: var(--track); overflow: hidden; }
.kpi .meter i { display: block; height: 100%; background: var(--accent); transition: width .6s ease; }
.right { display: flex; align-items: center; gap: 10px; flex: none; }

.diagram { position: relative; display: grid; grid-template-columns: 262px 124px minmax(0, 1fr);
  height: clamp(330px, calc(100vh - 56px - 304px), 404px); padding: 10px 14px 8px; }
.flow { position: absolute; inset: 0; width: 100%; height: 100%; pointer-events: none; z-index: 3; }
.endpoint { position: relative; display: flex; flex-direction: column; min-height: 0; border: 1px solid var(--line); border-radius: 10px;
  background: var(--panel); padding: 9px 10px 8px; }
.ehead { display: flex; align-items: baseline; gap: 8px; line-height: 20px; }
.ehead b { font-size: 14px; }
.ehead span { font-size: 12px; color: var(--mute); white-space: nowrap; }
.esub { font-size: 12px; color: var(--mute); line-height: 17px; margin-bottom: 6px; white-space: nowrap; }
.esub b { color: var(--fg); font-weight: 600; }
.qlist { position: relative; flex: 1; min-height: 0; overflow: hidden; }
.qchip { position: absolute; left: 0; right: 0; top: 0; height: 34px; display: grid; grid-template-columns: 3px 8px minmax(0, 1fr) auto;
  grid-template-rows: 17px 15px; column-gap: 6px; align-items: center; padding: 1px 7px 0 0; border-radius: 5px; background: var(--panel2);
  font-size: 12px; line-height: 17px; white-space: nowrap; overflow: hidden; transition: transform .45s cubic-bezier(.2, .8, .2, 1), opacity .35s; }
.qchip .wt { grid-row: 1 / 3; align-self: stretch; margin-top: -1px; }
.qchip .td { width: 8px; height: 8px; border-radius: 2px; }
.qchip .name { min-width: 0; overflow: hidden; }
.qchip .age { color: var(--fg); font-weight: 600; font-variant-numeric: tabular-nums; text-align: right; }
.qchip .meta { grid-column: 3 / 5; color: var(--mute); font-size: 11.5px; line-height: 15px; overflow: hidden; font-variant-numeric: tabular-nums; }
.qchip.dim { opacity: .28; }
.chip-enter-from, .chip-leave-to { opacity: 0; }
.chip-leave-active { position: absolute; }
.qmore { position: absolute; left: 0; right: 0; bottom: 0; height: 20px; font-size: 12px; color: var(--mute); text-align: center; background: var(--panel); }
.qempty { color: var(--mute); font-size: 12px; padding: 4px 0; }
.efoot { display: flex; align-items: flex-end; gap: 8px; margin-top: 6px; font-size: 12px; color: var(--mute); line-height: 15px; white-space: nowrap; }
.efoot canvas { flex: 1; height: 26px; min-width: 0; }
.efoot b { color: var(--fg); font-weight: 600; }
.nodes { display: grid; grid-template-rows: repeat(3, minmax(0, 1fr)); gap: 8px; min-width: 0; }
.bay { position: relative; min-width: 0; min-height: 0; }
.ghost { position: absolute; inset: 0; display: flex; align-items: center; justify-content: center; border: 1px dashed var(--line); border-radius: 10px;
  color: var(--mute); font-size: 13px; padding: 0 12px; text-align: center; }

.jobs { display: flex; flex-direction: column; min-height: 0; padding: 0 14px 6px; }
.jhead { display: flex; align-items: center; gap: 12px; height: 30px; flex: none; }
.jhead h2 { margin: 0; font-size: 13px; font-weight: 400; color: var(--mute); white-space: nowrap; }
.jhead h2 .name { font-size: 14px; font-weight: 700; color: var(--fg); }
.jhead .sum { font-size: 12px; color: var(--mute); font-variant-numeric: tabular-nums; white-space: nowrap; }
.jhead .sum b { color: var(--fg); font-weight: 600; }
.jhead .sum b.bad { color: var(--bad); }
.legend { margin-left: auto; display: flex; gap: 14px; font-size: 12px; color: var(--mute); white-space: nowrap; }
.legend span::before { content: ''; display: inline-block; width: 9px; height: 9px; border-radius: 2px; margin-right: 5px; background: var(--c); vertical-align: -1px; }
.fchip { display: inline-flex; align-items: center; gap: 6px; height: 24px; padding: 0 4px 0 10px; border-radius: 12px;
  background: color-mix(in srgb, var(--accent) 20%, transparent); color: var(--fg); font-size: 12px; white-space: nowrap; }
.fchip button { border: 0; background: transparent; color: var(--fg); cursor: pointer; min-width: 24px; min-height: 24px; border-radius: 12px; font-size: 13px; padding: 0 6px; }
.fchip button:hover { background: var(--hover); }
.fchip kbd { font: 11px var(--mono); color: var(--mute); }
.jtable { --cols: 62px 104px 70px minmax(150px, 1.15fr) 100px minmax(180px, 2.4fr) 54px 84px 70px; display: flex; flex-direction: column; flex: 1; min-height: 0; }
.jcols, .jrow { display: grid; grid-template-columns: var(--cols); column-gap: 10px; align-items: center; padding: 0 8px; }
.jcols { height: 22px; font-size: 12px; color: var(--mute); border-bottom: 1px solid var(--line); flex: none; }
.r { text-align: right; }
.jrows { position: relative; flex: 1; min-height: 0; overflow: hidden; }
.jrow { position: absolute; left: 0; right: 0; top: 0; height: 28px; border-bottom: 1px solid var(--rule); font-size: 13px; white-space: nowrap;
  transition: transform .5s cubic-bezier(.2, .8, .2, 1), opacity .4s; }
.jrow > * { overflow: hidden; text-overflow: ellipsis; }
.row-enter-from, .row-leave-to { opacity: 0; }
.row-leave-active { transition: opacity .2s; }
.jempty { color: var(--mute); font-size: 13px; padding: 6px 8px; }
.jobid { min-width: 24px; min-height: 22px; padding: 0 2px; margin-left: -2px; border: 0; background: none; color: var(--mute); font: inherit;
  font-variant-numeric: tabular-nums; text-decoration: underline; text-decoration-color: var(--ctl); text-underline-offset: 3px; cursor: pointer; }
.jobid:hover { color: var(--fg); }
.cmd { font-family: var(--mono); font-size: 12.5px; }
.wt i { display: inline-block; width: 8px; height: 8px; border-radius: 50%; margin-right: 6px; }
.tool i { display: inline-block; width: 9px; height: 9px; border-radius: 2px; margin-right: 6px; vertical-align: -1px; }
.nd { color: var(--mute); }
.num { font-variant-numeric: tabular-nums; }
.st { display: inline-flex; align-items: center; gap: 6px; font-size: 12px; font-weight: 600; }
.st i { width: 8px; height: 8px; border-radius: 50%; display: inline-block; background: var(--mute); }
.st.queued { color: var(--queue); } .st.queued i { background: var(--queue); }
.st.running { color: var(--accent); } .st.running i { background: var(--accent); animation: blink 1.2s ease-in-out infinite; }
.st.ok { color: var(--ok); } .st.ok i { background: var(--ok); }
.st.failed { color: var(--bad); } .st.failed i { background: var(--bad); }
.st.cancelled { color: var(--mute); }
@keyframes blink { 50% { opacity: .3; } }
@media (max-width: 1330px) {
  .diagram { grid-template-columns: 244px 104px minmax(0, 1fr); }
  .kpi { padding: 3px 8px; }
  .jtable { --cols: 60px 100px 66px minmax(140px, 1.1fr) 96px minmax(160px, 2.4fr) 52px 80px 64px; }
  .legend { display: none; }
}
@media (prefers-reduced-motion: reduce) {
  .cluster *, .cluster *::before, .cluster *::after { animation: none !important; }
  .qchip, .jrow, .kpi .meter i { transition-property: opacity, background-color !important; }
}
</style>
