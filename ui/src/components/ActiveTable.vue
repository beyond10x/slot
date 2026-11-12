<script setup lang="ts">
import { computed, reactive } from 'vue';
import { fmtBytes, fmtDur } from '../lib/format.ts';
import { cmdText, jobId, runS, waitS } from '../lib/job.ts';
import { phase } from '../lib/units.ts';
import { jobs, live, select, sparks, store } from '../store.ts';
import CoresSpark from './CoresSpark.vue';
import ToolDot from './ToolDot.vue';
import UnitChips from './UnitChips.vue';

/** Detail rows open by default for this many of the busiest jobs. */
const OPEN_BUSIEST = 3;

/** Explicit open/closed choices; jobs without one follow the busiest-3 default. */
const toggled = reactive(new Map<number, boolean>());

const rows = computed(() => {
  const t = store.clock;
  const running = [...jobs.values()].filter((j) => j.state === 'running').sort((a, b) => jobId(a) - jobId(b));
  const busiest = new Set(
    running
      .map((j) => jobId(j))
      .filter((id) => live.has(id))
      .sort((a, b) => (live.get(b)?.cores ?? 0) - (live.get(a)?.cores ?? 0))
      .slice(0, OPEN_BUSIEST),
  );
  return running.map((j) => {
    const id = jobId(j);
    const l = live.get(id);
    const run = runS(j, t);
    return {
      id,
      job: j,
      live: l,
      open: toggled.get(id) ?? busiest.has(id),
      cmd: cmdText(j),
      phase: phase(l?.units ?? []),
      wait: fmtDur(waitS(j, t)),
      elapsed: run === null ? '' : fmtDur(run),
      cores: l ? l.cores.toFixed(1) : '',
      rss: l ? fmtBytes(l.rss) : '',
      spark: [...sparks.get(id)],
    };
  });
});

function toggle(id: number, open: boolean): void {
  toggled.set(id, !open);
}
</script>

<template>
  <table>
    <thead>
      <tr>
        <th></th><th>#</th><th>tool</th><th>repo</th><th>command</th><th class="opt2">output</th><th>phase</th><th class="num">slots</th>
        <th class="num">wait</th><th class="num">elapsed</th><th class="num">cores</th><th class="opt">last 60 s</th>
        <th class="num opt">RSS</th><th class="opt">session</th>
      </tr>
    </thead>
    <tbody>
      <template v-for="r in rows" :key="r.id">
        <tr :class="{ sel: store.selected === r.id }" @click="select(r.id)">
          <td class="tg">
            <button type="button" :aria-expanded="r.open" :title="r.open ? 'hide units' : 'show units'"
              :aria-label="`${r.open ? 'hide' : 'show'} units of job ${r.id}`"
              @click.stop="toggle(r.id, r.open)">{{ r.open ? '▾' : '▸' }}</button>
          </td>
          <td><button type="button" class="jobid" :aria-label="`output of job ${r.id}`" @click.stop="select(r.id)">{{ r.id }}</button></td>
          <td><ToolDot :name="r.job.toolchain" /></td>
          <td class="repo" :title="r.job.repo">{{ r.job.repo }}</td>
          <td class="cmd" :title="r.cmd">{{ r.cmd }}</td>
          <td class="opt2">
            <button type="button" class="live" :class="{ on: store.selected === r.id }" title="watch this job's output live"
              @click.stop="select(r.id)">● live</button>
          </td>
          <td><span class="phase" :class="r.phase">{{ r.phase }}</span></td>
          <td class="num">{{ r.job.tokens }}</td>
          <td class="num">{{ r.wait }}</td>
          <td class="num">{{ r.elapsed }}</td>
          <td class="num">{{ r.cores }}</td>
          <td class="opt"><CoresSpark :points="r.spark" :t1="store.clock" :span="sparks.spanMs" /></td>
          <td class="num opt">{{ r.rss }}</td>
          <td class="muted opt">{{ r.job.session.slice(0, 8) }}</td>
        </tr>
        <tr v-if="r.open" class="detail">
          <td colspan="14"><UnitChips :live="r.live" /></td>
        </tr>
      </template>
      <tr v-if="!rows.length" class="empty"><td colspan="14" class="muted">nothing running</td></tr>
    </tbody>
  </table>
</template>

<style scoped>
td.tg { width: 22px; padding-right: 0; }
td.tg button { min-width: 24px; min-height: 24px; border: 0; background: none; padding: 0 4px; cursor: pointer; color: var(--mute); }
button.live { font-size: 12px; min-height: 24px; padding: 0 6px; border: 1px solid #e5a5a2; border-radius: 3px; background: #fff; color: #c4302b;
  cursor: pointer; }
button.live:hover, button.live.on { background: #c4302b; border-color: #c4302b; color: #fff; }
tr.detail { cursor: default; }
tr.detail td { background: transparent; border-bottom: 1px solid #eee; padding-top: 0; }
.phase { font-size: 12px; padding: 0 5px; border-radius: 3px; background: #eee; }
.phase.compiling { background: #dbeaf5; }
.phase.testing { background: #d6efe6; }
.phase.linking { background: #e4e4e4; }
</style>
