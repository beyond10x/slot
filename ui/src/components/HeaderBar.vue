<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, useTemplateRef, watch } from 'vue';
import { fmtDur } from '../lib/format.ts';
import { HEADROOM_PRESETS, HEADROOM_STEP, MAX_HEADROOM, headroomCaption, machineSentence } from '../lib/headroom.ts';
import { jobKey, jobName } from '../lib/job.ts';
import { failedStats, queueSummary, servedQueue } from '../lib/queue.ts';
import { sharesLine } from '../lib/shares.ts';
import { failText, failedHour, failedOf } from '../lib/cluster.ts';
import { census, jobs, nodes, select, setHeadroom, store } from '../store.ts';

const WINDOWS: ReadonlyArray<[number, string]> = [
  [120, '2 min'],
  [600, '10 min'],
  [1800, '30 min'],
  [3600, '1 h'],
  [21600, '6 h'],
];

/** The first state has arrived: until then the header shows its name and keeps the room of its numbers. */
const loaded = computed(() => store.tokens > 0);
const limit = computed(() => store.limit || store.tokens);
const used = computed(() => Math.max(0, limit.value - store.free));
const withheld = computed(() => Math.max(0, store.tokens - limit.value));
const connection = computed(() => {
  if (store.connected) return { word: 'connected', title: 'connected to the daemon: the page follows it second by second' };
  if (!loaded.value) return { word: 'connecting…', title: 'waiting for the first answer of the daemon' };
  return { word: 'not connected', title: 'not connected to the daemon: the numbers are the last ones received' };
});

/** Slider value; follows the daemon except while the user moves it. */
const draft = ref(store.headroom);
let editedAt = 0;
watch(
  () => store.headroom,
  (v) => {
    if (Date.now() - editedAt > 1500) draft.value = v;
  },
);
// A refused change puts the slider back on what the daemon runs, so the caption never claims it.
watch(
  () => store.headroomRefused,
  () => {
    draft.value = store.headroom;
  },
);
function pick(percent: number): void {
  editedAt = Date.now();
  draft.value = percent;
  setHeadroom(percent);
}
const onHeadroom = (e: Event): void => pick(Number((e.target as HTMLInputElement).value));
const caption = computed(() =>
  headroomCaption(store.cores || store.tokens, store.tokens, draft.value, store.machine.headroom_base),
);
const machine = computed(() => machineSentence(store.machine, store.cores, store.cpuQuota));
const shares = computed(() => sharesLine(store.shares));
const call = computed(() =>
  store.callApps.length ? `call (${store.callApps.join(', ')}): headroom raised, back 2 min after the call` : '',
);
const running = computed(() => {
  let n = 0;
  for (const j of jobs.values()) if (j.state === 'running') n++;
  return n;
});
const queue = computed(() => queueSummary(servedQueue(jobs.values()), store.clock));
/**
 * Failed jobs of the last 60 minutes over every node, as each node counts them, split by cause; the button opens the output of the
 * listed one that ended last.
 */
const failed = computed(() => {
  const by = failedHour(nodes.value, jobs.values(), store.clock);
  const n = failedOf(by);
  if (!n) return null;
  const f = failedStats(jobs.values(), store.clock);
  return { n, why: failText(by), id: f.newest ? jobKey(f.newest) : null, name: f.newest ? jobName(f.newest) : '' };
});

/** The explanation of every item of the header: open by a click or Enter on "?", or while the pointer is on it. */
const pinned = ref(false);
const hovered = ref(false);
const help = computed(() => pinned.value || hovered.value);
const helpBtn = useTemplateRef<HTMLButtonElement>('helpBtn');
function toggleHelp(): void {
  pinned.value = !pinned.value;
  hovered.value = false;
}
function onEscape(e: KeyboardEvent): void {
  if (!help.value) return;
  pinned.value = false;
  hovered.value = false;
  e.preventDefault();
  helpBtn.value?.focus();
}
function onOutside(e: MouseEvent): void {
  if (pinned.value && e.target instanceof Node && !el.value?.contains(e.target)) pinned.value = false;
}

const el = useTemplateRef<HTMLElement>('el');
let ro: ResizeObserver | null = null;
onMounted(() => {
  document.addEventListener('click', onOutside);
  const h = el.value;
  if (!h) return;
  ro = new ResizeObserver(() => document.documentElement.style.setProperty('--hdr', h.offsetHeight + 'px'));
  ro.observe(h);
});
onUnmounted(() => {
  document.removeEventListener('click', onOutside);
  ro?.disconnect();
});
</script>

<template>
  <header ref="el" @keydown.esc="onEscape">
    <div class="row1" :class="{ pending: !loaded }">
      <div class="tile brand">
        <h1><span class="name">slot</span><span class="sr"> · </span><span class="host" :title="`${store.host} · ${store.cores} cores`">{{ store.host }}<template v-if="store.cores"> · {{ store.cores }} cores</template></span></h1>
        <span class="conn" data-ux-region="connection" :class="{ ok: store.connected, wait: !loaded }" :title="connection.title"><i></i>{{ connection.word }}</span>
      </div>
      <span class="tile" data-ux-region="slots">
        <span class="k">slots in use</span>
        <span class="v"><span class="tokens" aria-hidden="true"><i v-for="k in store.tokens" :key="k" :class="{ on: k <= used, off: k > limit }"></i></span> <b>{{ used }}/{{ limit }}</b></span>
      </span>
      <span class="tile headroom" data-ux-region="headroom">
        <label class="k" for="headroom">headroom for you</label>
        <span class="v">
          <input id="headroom" type="range" min="0" :max="MAX_HEADROOM" :step="HEADROOM_STEP" :value="draft"
            aria-describedby="headroom-caption" @input="onHeadroom" /> <b class="pc">{{ draft }} %</b>
          <span class="presets" role="group" aria-label="headroom presets">
            <button v-for="p in HEADROOM_PRESETS" :key="p" type="button" class="btn" :aria-pressed="draft === p" @click="pick(p)">{{ p }} %</button>
          </span>
        </span>
      </span>
      <span class="tile"><span class="k">running</span> <b class="v">{{ running }}</b></span>
      <span class="pair" data-ux-region="queue">
        <span class="tile"><span class="k">queue</span> <b class="v">{{ queue.length }}</b></span>
        <span class="tile"><span class="k">longest wait</span> <b class="v">{{ queue.length ? fmtDur(queue.longestS) : '–' }}</b></span>
      </span>
      <button v-if="failed?.id" type="button" class="tile failed"
        :aria-label="`failed ${failed.n} (60 min), ${failed.why}: output of job ${failed.name}, the newest failed job`"
        @click="select(failed.id, $event)"><span class="k">failed</span> <span class="v"><b>{{ failed.n }}</b> <span class="span">(60 min)</span> <span class="why" data-fail-causes>{{ failed.why }}</span></span></button>
      <span v-else-if="failed" class="tile failed"><span class="k">failed</span> <span class="v"><b>{{ failed.n }}</b> <span class="span">(60 min)</span> <span class="why" data-fail-causes>{{ failed.why }}</span></span></span>
      <span v-if="census" class="tile"><span class="k">not on slot</span> <span class="v"><b>{{ census.unmanaged_cores.toFixed(1) }}</b> <span class="span">cores</span></span></span>
      <span class="end">
        <a class="btn navlink" href="#cluster" title="the cluster start page: nodes, queue and the newest jobs">Cluster</a>
        <label class="tile"><span class="k">chart window</span>
          <span class="v"><select v-model.number="store.windowS">
            <option v-for="[s, label] in WINDOWS" :key="s" :value="s">{{ label }}</option>
          </select></span></label>
        <button ref="helpBtn" type="button" class="btn helpbtn" aria-label="what the header shows" :aria-expanded="help" aria-controls="header-help"
          @click="toggleHelp" @mouseenter="hovered = true" @mouseleave="hovered = false">?</button>
      </span>
    </div>
    <div class="row2">
      <span v-if="loaded" id="headroom-caption" class="cap">{{ caption }}</span>
      <span v-if="store.error" class="chip err" :title="store.error">{{ store.error }}</span>
      <span v-if="store.headroomError" class="chip err" role="status" :title="store.headroomError">{{ store.headroomError }}</span>
      <span v-if="shares" class="chip shares" :class="{ over: store.shares?.braking }" data-ux-region="shares" :title="shares">{{ shares }}</span>
      <span v-if="machine" class="chip machine" :class="{ over: store.machine.over_target }" data-ux-region="machine" :title="machine">{{ machine }}</span>
      <span v-if="store.upgrade" class="chip upgrade" data-ux-region="upgrade" role="status" :title="store.upgrade">{{ store.upgrade }}</span>
      <span v-if="call" class="chip call" :title="call">{{ call }}</span>
    </div>
    <div v-if="help" id="header-help" class="help" role="region" aria-label="what the header shows"
      @mouseenter="hovered = true" @mouseleave="hovered = false">
      <dl>
        <div><dt>connected</dt><dd>The page follows the daemon second by second. "Not connected": the numbers are the last ones received.</dd></div>
        <div><dt>slots in use</dt><dd>Slots that running jobs hold, of the slots builds may use now. A dashed box is a slot the headroom keeps back{{ withheld ? ` (${withheld} now)` : '' }}.</dd></div>
        <div><dt>headroom for you</dt><dd>Share of the CPU kept free for you (browser, video calls). Builds get the rest: fewer slots, and one CPU limit on all build jobs together. The line below the numbers says what that comes to.</dd></div>
        <div><dt>running, queue, longest wait</dt><dd>Jobs that hold slots, jobs that wait, and how long the oldest of them has waited. The Queue table names what each one waits for.</dd></div>
        <div><dt>failed</dt><dd>Jobs that ended with an error in the last 60 min on every node, by cause: test (the command failed), timeout (ended at its run-time limit, exit 124), infra (a lost connection, exit 3; a command that could not run or was not found, 125 or 127). The button opens the output of the newest one.</dd></div>
        <div><dt>not on slot</dt><dd>Cores used by builds that do not run through slot. The Host section shows which sessions run them.</dd></div>
        <div><dt>chart window</dt><dd>The time span of every chart of the page.</dd></div>
        <div v-if="machine"><dt>machine</dt><dd>The headroom counts the whole machine: builds get the target less what other programs use, at least 2 CPUs. "Over target": builds are at that lowest limit and the machine is still busier. The Host section lists the other programs.</dd></div>
        <div v-if="shares"><dt>by share</dt><dd>Jobs start by the CPUs they are expected to use, not by slots: a job starts while the CPUs the pool and the compile cache used over the last 5 s, plus the shares of jobs that just started, leave room for its share in the budget. The budget grows while jobs run as fast as before and shrinks when they slow down. "Idle lend": jobs that use almost no CPU leave their share to the queue. The share column shows what a job uses of the share it asked for.</dd></div>
        <div v-if="store.upgrade"><dt>upgrade pending</dt><dd>The daemon starts its new version at the first moment no job runs; until then it may hold new jobs back.</dd></div>
        <div v-if="call"><dt>call</dt><dd>A call uses the microphone or the camera, so the headroom is raised; it goes back 2 min after the call. Moving the slider now keeps your value.</dd></div>
      </dl>
    </div>
  </header>
</template>

<style scoped>
/*
 * Two rows of fixed height: the numbers, then one line for what the headroom comes to and for status lines.
 * A status line that appears takes room in the second row, so the page below never moves.
 */
header { position: sticky; top: 0; z-index: 2; padding: 7px 14px 6px; border-bottom: 1px solid var(--line); background: var(--bg); }
.row1 { display: flex; flex-wrap: wrap; align-items: flex-start; gap: 6px 20px; min-height: 37px; }
.row1.pending > :not(.brand) { visibility: hidden; }
.tile { display: inline-flex; flex-direction: column; justify-content: space-between; height: 37px; min-width: 0; }
.pair, .end { display: inline-flex; gap: 14px; }
.end { margin-left: auto; align-items: flex-end; gap: 10px; }
.k { font-size: 12px; line-height: 15px; color: var(--mute); white-space: nowrap; }
.v { display: inline-flex; align-items: center; gap: 6px; height: 22px; font-size: 13px; line-height: 22px; white-space: nowrap; }
b, b.v { font-size: 15px; font-weight: 700; font-variant-numeric: tabular-nums; }
.span { color: var(--mute); }
.brand { display: grid; grid-template-columns: auto auto; justify-content: start; align-content: space-between; column-gap: 8px; }
h1 { display: contents; margin: 0; font-size: inherit; font-weight: inherit; }
h1 .name { font-size: 16px; font-weight: 700; line-height: 22px; }
h1 .host { grid-row: 2; grid-column: 1 / -1; max-width: 200px; overflow: hidden; text-overflow: ellipsis; font-size: 12px; line-height: 15px;
  color: var(--mute); white-space: nowrap; }
.conn { align-self: center; font-size: 12px; line-height: 15px; color: var(--mute); white-space: nowrap; }
.conn i { display: inline-block; width: 8px; height: 8px; margin-right: 4px; border-radius: 50%; background: #c62828; }
.conn.ok i { background: #1e8e3e; }
.conn.wait i { background: #b0b0b0; }
.conn:not(.ok):not(.wait) { color: var(--bad); font-weight: 600; }
.tokens { display: inline-flex; gap: 2px; }
.tokens i { width: 5px; height: 14px; background: #dcdcdc; border-radius: 1px; }
.tokens i.on { background: var(--ink); }
.tokens i.off { background: transparent; outline: 1px dashed #9a9a9a; outline-offset: -1px; }
.headroom input { width: 88px; }
.pc { min-width: 40px; text-align: right; }
.presets { display: inline-flex; gap: 3px; margin-left: 2px; }
.presets .btn { padding: 0 6px; font-variant-numeric: tabular-nums; }
.failed { padding: 0 8px; margin: 0 -8px; border: 1px solid #e0a09c; border-radius: 4px; background: #fff; color: var(--bad); cursor: pointer;
  text-align: left; }
.failed:hover { background: #fdf1f0; }
.failed .k, .failed .span { color: var(--bad); }
.failed .why { font-size: 12px; color: var(--bad); }
.navlink { display: inline-flex; align-items: center; text-decoration: none; }
.helpbtn { border-radius: 50%; padding: 0; width: 24px; font-weight: 700; }
.row2 { display: flex; align-items: center; gap: 10px; height: 24px; margin-top: 5px; overflow: hidden; font-size: 13px; white-space: nowrap; }
.cap { flex: none; color: var(--mute); font-variant-numeric: tabular-nums; }
.chip { flex: 0 1 auto; min-width: 0; padding: 0 8px; border-radius: 10px; overflow: hidden; text-overflow: ellipsis; line-height: 20px;
  font-variant-numeric: tabular-nums; }
.chip.err { flex-shrink: 0; background: #fde8e6; color: #8a1c12; font-weight: 600; }
.chip.machine { background: #eef1f4; color: #2b3a48; }
.chip.machine.over { background: #fde8e6; color: #8a1c12; }
.chip.shares { background: #eef4ee; color: #24452a; }
.chip.shares.over { background: #fde8e6; color: #8a1c12; }
.chip.upgrade { background: #fff3d6; color: #6b4500; }
.chip.call { flex-shrink: 1000; background: #e6f0ff; color: #1d4f91; }
.help { position: absolute; top: calc(100% + 4px); right: 14px; z-index: 3; width: min(640px, calc(100vw - 28px)); padding: 10px 14px;
  border: 1px solid var(--ctl); border-radius: 6px; background: #fff; box-shadow: 0 6px 18px rgb(0 0 0 / 14%); font-size: 13px; }
dl { margin: 0; display: grid; gap: 6px; }
dl > div { display: grid; grid-template-columns: 150px minmax(0, 1fr); gap: 0 12px; }
dt { font-weight: 600; }
dd { margin: 0; color: var(--ink); }
/* A narrow window wraps the first row and lets a long status line take a second line; the page keeps its place by the observed height. */
@media (max-width: 1100px) {
  .row2 { flex-wrap: wrap; height: auto; min-height: 24px; gap: 4px 10px; white-space: normal; }
  .chip { white-space: normal; }
}
</style>
