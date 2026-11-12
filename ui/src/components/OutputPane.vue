<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, useTemplateRef, watch } from 'vue';
import type { Chunk } from '../generated/types.ts';
import { Terminal, lineHtml, lineIsErr } from '../lib/ansi.ts';
import type { Segment } from '../lib/ansi.ts';
import { cmdText, isActive } from '../lib/job.ts';
import { atBottom } from '../lib/scroll.ts';
import { jobs, select, store } from '../store.ts';

/** Most lines kept in the pane; older ones are dropped and counted. */
const MAX_LINES = 5000;

const props = defineProps<{ id: number }>();

const job = computed(() => jobs.get(props.id));
const title = computed(() => {
  const j = job.value;
  return j ? `#${props.id} ${j.repo}: ${cmdText(j)}` : `#${props.id}`;
});

type Stream = 'connecting' | 'live' | 'ended' | 'unavailable';
const stream = ref<Stream>('connecting');
const status = computed(() => {
  if (stream.value === 'live') return '● live';
  if (stream.value === 'connecting') return 'connecting…';
  if (stream.value === 'unavailable') return 'output not available';
  const j = job.value;
  if (!j || isActive(j.state)) return 'ended';
  return j.state === 'cancelled' ? 'ended (cancelled)' : `ended (exit ${j.exit})`;
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

onMounted(() => {
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
onUnmounted(() => es?.close());
</script>

<template>
  <aside class="pane">
    <div class="bar">
      <b class="title" :title="title">{{ title }}</b>
      <span class="status" :class="stream">{{ status }}</span>
      <span class="controls">
        <label title="keep the newest output in view"><input v-model="follow" type="checkbox" /> follow</label>
        <label title="switch to the most recently started running job by itself">
          <input v-model="store.followNewest" type="checkbox" /> follow newest</label>
        <button type="button" @click="select(null)">close</button>
      </span>
    </div>
    <pre ref="pre" @scroll.passive="onScroll"><div ref="dropped" class="dropped" :hidden="!dropped">… {{ dropped }} earlier lines dropped</div></pre>
  </aside>
</template>

<style scoped>
.pane { border-left: 1px solid var(--line); background: var(--panel); height: calc(100vh - var(--hdr)); position: sticky;
  top: var(--hdr); display: flex; flex-direction: column; overflow: hidden; }
.bar { display: flex; flex-wrap: wrap; align-items: center; gap: 4px 10px; padding: 8px 12px; border-bottom: 1px solid var(--line); }
.title { flex: 1 1 12em; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.status { font-size: 12px; white-space: nowrap; color: var(--mute); }
.status.live { color: #c4302b; font-weight: 600; }
.controls { display: inline-flex; align-items: center; gap: 10px; margin-left: auto; white-space: nowrap; }
.controls label { display: inline-flex; align-items: center; gap: 3px; }
pre { flex: 1; margin: 0; padding: 8px 12px 8px 9px; overflow: auto; font: 12px/1.35 ui-monospace, monospace; white-space: pre-wrap;
  word-break: break-all; color: #1a1a1a; }
pre :deep(.ln) { border-left: 2px solid transparent; padding-left: 3px; }
pre :deep(.ln:empty::before) { content: '\200b'; }
pre :deep(.ln.err) { border-left-color: #e5a5a2; }
pre :deep(.ln.note) { color: var(--mute); font-style: italic; }
.dropped { color: var(--mute); font-style: italic; padding-left: 5px; }
</style>
