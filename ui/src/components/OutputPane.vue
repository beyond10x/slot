<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, useTemplateRef, watch } from 'vue';
import type { Chunk } from '../generated/types.ts';
import { Terminal, lineHtml, lineIsErr } from '../lib/ansi.ts';
import type { Segment } from '../lib/ansi.ts';
import { fmtBytes, fmtClock, fmtDur } from '../lib/format.ts';
import { checkoutLabel, cmdText, jobName, resultLong, runS, times, waitS } from '../lib/job.ts';
import { levelText, queuePlace, servedQueue, waitReasonText } from '../lib/queue.ts';
import { atBottom, revealDelta } from '../lib/scroll.ts';
import { jobs, liveOf, select, sessionTitles, store } from '../store.ts';

/** Most lines kept in the pane; older ones are dropped and counted. */
const MAX_LINES = 5000;

const props = defineProps<{ id: string }>();

const job = computed(() => jobs.get(props.id));
/** "#42 ledger · worktree tax-rounding: " before the command; the number alone while the job is unknown. */
const head = computed(() => (job.value ? `#${jobName(job.value)} ${checkoutLabel(job.value)}: ` : `#${props.id}`));
const cmd = computed(() => (job.value ? cmdText(job.value) : ''));

type Stream = 'connecting' | 'live' | 'ended' | 'unavailable';
const stream = ref<Stream>('connecting');
/** A queued job's place and wait; null once it runs. */
const queued = computed(() => {
  const j = job.value;
  if (!j || j.state !== 'queued') return null;
  const place = queuePlace(servedQueue(jobs.values()), props.id);
  return { place: place ? `#${place.pos} of ${place.of}` : '', wait: fmtDur(waitS(j, store.clock)), why: waitReasonText(j) };
});
/**
 * The job's state in words, then the stream's when it adds something. A queued job has an open stream
 * with nothing in it, so "live" is said only of a running job whose stream is open.
 */
const status = computed(() => {
  const j = job.value;
  const q = queued.value;
  if (q) return `queued${q.place ? `, ${q.place}` : ''}, waiting ${q.wait}`;
  const output = stream.value === 'unavailable' ? ', output not available' : stream.value === 'connecting' ? ', loading output…' : '';
  if (!j) return stream.value === 'unavailable' ? 'output not available' : 'loading output…';
  if (j.state === 'running') return stream.value === 'live' ? 'running, output live' : `running${output}`;
  return resultLong(j) + output;
});
/** Every value of the job the tables may cut or leave out at this width, as label and value. */
const facts = computed((): Array<[string, string]> => {
  const j = job.value;
  if (!j) return [];
  const t = times(j);
  const l = liveOf(j);
  const end = t.ended ?? store.clock;
  const run = runS(j, end);
  const title = sessionTitles.get(j.session);
  const out: Array<[string, string]> = [['in', j.cwd]];
  if (j.session) out.push(['session', title ? `${title} (${j.session})` : j.session]);
  out.push(['tool', j.toolchain], ['level', levelText(j)], ['slots', String(j.tokens)]);
  out.push([t.started === null && t.ended === null ? 'waiting' : 'waited', fmtDur(waitS(j, end))]);
  if (run !== null) out.push([t.ended === null ? 'running' : 'ran', fmtDur(run)]);
  if (l) out.push(['cores', l.cores.toFixed(1)], ['RSS', fmtBytes(l.rss) || '0 MB']);
  if (t.ended !== null) {
    out.push(['ended', fmtClock(t.ended)]);
    if (j.cpu_s) out.push(['CPU', `${j.cpu_s.toFixed(0)} s`]);
    if (Number(j.peak_rss)) out.push(['peak RSS', fmtBytes(Number(j.peak_rss))]);
  }
  return out;
});

const pre = useTemplateRef<HTMLPreElement>('pre');
const droppedEl = useTemplateRef<HTMLDivElement>('dropped');
const dropped = ref(0);
let term = new Terminal();
let cur: HTMLDivElement | null = null;
let lineCount = 0;
let es: EventSource | null = null;

/** Keep the newest output in view; off once the user scrolls up, on again at the bottom or by the toggle. */
const follow = ref(true);

function toEnd(): void {
  const p = pre.value;
  if (p) p.scrollTop = p.scrollHeight;
}

function onScroll(): void {
  const p = pre.value;
  if (p) follow.value = atBottom(p.scrollTop, p.clientHeight, p.scrollHeight);
}

watch(follow, (on) => {
  if (on) toEnd();
});

function newLine(p: HTMLPreElement): HTMLDivElement {
  const el = document.createElement('div');
  el.className = 'ln';
  p.appendChild(el);
  lineCount++;
  return el;
}

function paint(el: HTMLDivElement, line: readonly Segment[]): void {
  // lineHtml escapes every text; styles come only from the parser's own colour table.
  el.innerHTML = lineHtml(line);
  el.classList.toggle('err', lineIsErr(line));
}

function trim(): void {
  while (lineCount > MAX_LINES) {
    const first = droppedEl.value?.nextElementSibling;
    if (!first || first === cur) break;
    first.remove();
    lineCount--;
    dropped.value++;
  }
}

function write(text: string, err: boolean): void {
  const p = pre.value;
  if (!p) return;
  const { done, current } = term.write(text, err);
  cur ??= newLine(p);
  for (const line of done) {
    paint(cur, line);
    cur = newLine(p);
  }
  paint(cur, current);
  trim();
  if (follow.value) toEnd();
}

function note(text: string): void {
  const p = pre.value;
  if (!p) return;
  const el = newLine(p);
  el.className = 'ln note';
  el.textContent = text;
  cur = null;
  term = new Terminal();
  if (follow.value) toEnd();
}

function reset(): void {
  const p = pre.value;
  const d = droppedEl.value;
  if (p && d) p.replaceChildren(d);
  term = new Terminal();
  cur = null;
  lineCount = 0;
  dropped.value = 0;
}

function endMessage(data: string): string {
  try {
    const d = JSON.parse(data) as { msg?: unknown };
    return typeof d.msg === 'string' ? d.msg : '';
  } catch {
    return '';
  }
}

const paneEl = useTemplateRef<HTMLElement>('paneEl');
const closeEl = useTemplateRef<HTMLButtonElement>('closeEl');
/** The control that held the focus when the pane was asked for; closing by the keyboard gives the focus back to it. */
let opener: HTMLElement | null = null;

function noteOpener(): void {
  const a = document.activeElement;
  if (a instanceof HTMLElement && a !== document.body && !paneEl.value?.contains(a)) opener = a;
}

/** Asked for by the keyboard, the pane takes the focus on "close"; the output is one Tab further. */
function takeFocus(): void {
  if (!store.paneFocus || store.selected !== props.id) return;
  store.paneFocus = false;
  noteOpener();
  closeEl.value?.focus({ preventScroll: true });
}
watch(() => store.paneFocus, takeFocus, { flush: 'post' });

/** Closes the pane; `refocus` puts the focus back on the control that opened it. */
function close(refocus: boolean): void {
  const o = opener;
  select(null);
  if (refocus && o?.isConnected) o.focus();
}

/** The Enter that opened the pane, still held down, must not press "close": a repeated key does nothing there. */
function noRepeat(e: KeyboardEvent): void {
  if (e.repeat) e.preventDefault();
}

/** Escape closes the pane from anywhere on the page, but not while it closes the list of a select. */
function onKey(e: KeyboardEvent): void {
  if (e.key !== 'Escape' || e.defaultPrevented || e.target instanceof HTMLSelectElement) return;
  close(true);
}

/** As a bottom sheet (narrow windows) the pane can cover the control that opened it: scroll that control above the sheet. */
function keepOpenerInSight(): void {
  const pane = paneEl.value;
  if (!pane || !opener) return;
  if (getComputedStyle(pane).position !== 'fixed') return;
  const hdr = document.querySelector('header')?.getBoundingClientRect().bottom ?? 0;
  const r = opener.getBoundingClientRect();
  const dy = revealDelta(r.top, r.bottom, hdr, pane.getBoundingClientRect().top);
  if (dy) window.scrollBy(0, dy);
}

onMounted(() => {
  noteOpener();
  keepOpenerInSight();
  takeFocus();
  document.addEventListener('keydown', onKey);
  const j = job.value;
  if (j?.from_node) {
    note(`[the output of a job on ${j.node} stays on that node; the slot command that started it prints it]`);
    stream.value = 'unavailable';
    return;
  }
  const src = new EventSource(`api/jobs/${props.id}/output`);
  es = src;
  src.onopen = () => {
    // The server replays the kept tail on every connection, so a reconnect starts clean.
    reset();
    stream.value = 'live';
  };
  src.onmessage = (ev: MessageEvent<string>) => {
    const c = JSON.parse(ev.data) as Chunk;
    write(c.b, c.t === 'err');
  };
  src.addEventListener('end', (ev: MessageEvent<string>) => {
    const msg = endMessage(ev.data);
    if (msg) note(`[${msg}]`);
    stream.value = 'ended';
    src.close();
  });
  src.onerror = () => {
    if (src.readyState === EventSource.CLOSED) stream.value = 'unavailable';
    else stream.value = 'connecting';
  };
});
onUnmounted(() => {
  document.removeEventListener('keydown', onKey);
  es?.close();
});
</script>

<template>
  <aside ref="paneEl" class="pane" aria-label="job output">
    <div class="bar">
      <b class="title">{{ head }}<span v-if="job" class="cmd">{{ cmd }}</span></b>
      <p v-if="facts.length" class="facts">
        <span v-for="[k, v] in facts" :key="k" :class="{ wide: k === 'in' || k === 'session' }"><span class="k">{{ k }}</span> {{ v }}</span>
      </p>
      <div class="row">
        <span class="status" :class="job?.state">{{ status }}</span>
        <span class="controls">
          <label title="keep the newest output in view"><input v-model="follow" type="checkbox" /> follow</label>
          <label title="switch to the most recently started running job by itself">
            <input v-model="store.followNewest" type="checkbox" /> follow newest</label>
          <button ref="closeEl" type="button" class="btn" title="close the output (Escape)" @keydown="noRepeat" @click="close($event.detail === 0)">close</button>
        </span>
      </div>
    </div>
    <p v-if="queued" class="notyet">
      Not started yet: the output appears here when the job gets its slots.<template v-if="queued.why"> It {{ queued.why }}.</template>
    </p>
    <pre ref="pre" tabindex="0" role="log" aria-live="off" :aria-label="`output of job ${job ? jobName(job) : props.id}`" @scroll.passive="onScroll"><div ref="dropped" class="dropped" :hidden="!dropped">… {{ dropped }} earlier lines dropped</div></pre>
  </aside>
</template>

<style scoped>
.pane { border-left: 1px solid var(--line); background: var(--panel); height: calc(100vh - var(--hdr)); position: sticky;
  top: var(--hdr); display: flex; flex-direction: column; overflow: hidden; }
@media (max-width: 1099px) {
  .pane { position: fixed; top: auto; left: 0; right: 0; bottom: 0; height: 45vh; z-index: 1; border-left: 0;
    border-top: 1px solid var(--line); box-shadow: 0 -2px 6px rgb(0 0 0 / 8%); }
}
/* The bar names the job in full: its command wraps and is never cut, and its values stand below as label and value. */
.bar { display: grid; gap: 5px; padding: 9px 12px 8px; border-bottom: 1px solid var(--line); background: #fff; }
.title { font-size: 14px; line-height: 20px; overflow-wrap: anywhere; white-space: pre-wrap; }
.title .cmd { font: 400 13px/20px var(--mono); }
.facts { display: flex; flex-wrap: wrap; gap: 1px 14px; margin: 0; font-size: 13px; line-height: 19px; font-variant-numeric: tabular-nums;
  overflow-wrap: anywhere; }
.facts .wide { flex: 0 0 100%; }
.facts .k { color: var(--mute); }
.row { display: flex; flex-wrap: wrap; align-items: center; gap: 4px 12px; }
.status { font-size: 13px; font-weight: 600; font-variant-numeric: tabular-nums; }
.status.failed { color: var(--bad); }
.status.queued, .status.cancelled { color: var(--ink); }
.controls { display: inline-flex; align-items: center; gap: 12px; margin-left: auto; font-size: 13px; white-space: nowrap; }
.controls label { display: inline-flex; align-items: center; gap: 4px; min-height: 24px; cursor: pointer; }
.notyet { margin: 0; padding: 10px 12px 0; font-size: 13px; color: var(--mute); }
pre { flex: 1; margin: 0; padding: 8px 12px 8px 9px; overflow: auto; font: 13px/1.4 var(--mono); white-space: pre-wrap;
  word-break: break-all; color: #1a1a1a; }
pre :deep(.ln) { border-left: 2px solid transparent; padding-left: 3px; }
pre :deep(.ln:empty::before) { content: '\200b'; }
pre :deep(.ln.err) { border-left-color: #e5a5a2; }
pre :deep(.ln.note) { color: var(--mute); font-style: italic; }
.dropped { color: var(--mute); font-style: italic; padding-left: 5px; }
</style>
