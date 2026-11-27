<script setup lang="ts">
import { onMounted, onUnmounted, ref, watch } from 'vue';
import type { By, Usage } from '../generated/types.ts';
import { usageRows, usageUrl } from '../lib/usage.ts';
import type { UsageLine, UsageWindow } from '../lib/usage.ts';

const windows: [UsageWindow, string][] = [['today', 'today'], ['24h', 'last 24 h'], ['7d', 'last 7 days']];
const groups: [By, string][] = [['project', 'project'], ['repo', 'repository'], ['session', 'session'], ['node', 'node']];

const win = ref<UsageWindow>('today');
const by = ref<By>('project');
const rows = ref<UsageLine[]>([]);
const total = ref('');
const failed = ref('');

async function load(): Promise<void> {
  try {
    const res = await fetch(usageUrl(win.value, by.value, new Date()));
    if (!res.ok) throw new Error(`${res.status}`);
    const u = (await res.json()) as Usage;
    rows.value = usageRows(u, 15);
    total.value = `${u.total.jobs} jobs, ${(u.total.slot_s / 3600).toFixed(1)} slot-hours, ${(u.total.cpu_s / 3600).toFixed(1)} CPU-hours`;
    failed.value = '';
  } catch (e) {
    failed.value = `usage not available from this daemon (${String(e)})`;
  }
}

let timer: ReturnType<typeof setInterval> | undefined;
onMounted(() => {
  void load();
  timer = setInterval(() => void load(), 60_000);
});
onUnmounted(() => clearInterval(timer));
watch([win, by], () => void load());
</script>

<template>
  <div class="controls">
    <fieldset>
      <legend class="sr">window</legend>
      <label v-for="[v, l] in windows" :key="v"><input v-model="win" type="radio" name="usage-window" :value="v" /> {{ l }}</label>
    </fieldset>
    <fieldset>
      <legend class="sr">group by</legend>
      <label v-for="[v, l] in groups" :key="v"><input v-model="by" type="radio" name="usage-by" :value="v" /> {{ l }}</label>
    </fieldset>
    <span class="total" :class="{ bad: !!failed }">{{ failed || total }}</span>
  </div>
  <table>
    <thead>
      <tr>
        <th class="c-label">{{ by }}</th><th>share of slot time</th><th class="c-n num">slot-h</th><th class="c-w num">CPU share</th>
        <th class="c-n num">jobs</th><th class="c-w num opt">median wait</th>
      </tr>
    </thead>
    <tbody>
      <tr v-for="r in rows" :key="r.key" class="static">
        <td class="label" :title="by === 'session' && r.key ? `${r.label}\nsession ${r.key}` : r.label">{{ r.label }}</td>
        <td><span class="bar"><span class="track"><span class="fill" :style="{ width: `${r.width}%` }"></span></span><span class="share">{{ r.share }}</span></span></td>
        <td class="num">{{ r.slotHours }}</td>
        <td class="num">{{ r.cpuShare }}</td>
        <td class="num">{{ r.jobs }}</td>
        <td class="num opt">{{ r.wait }}</td>
      </tr>
      <tr v-if="!rows.length && !failed" class="empty"><td colspan="6">no finished jobs in this window</td></tr>
    </tbody>
  </table>
</template>

<style scoped>
.controls { display: flex; flex-wrap: wrap; gap: 4px 20px; align-items: center; margin: 2px 0 6px; font-size: 13px; }
fieldset { border: 0; margin: 0; padding: 0; display: flex; gap: 12px; }
fieldset label { display: inline-flex; align-items: center; gap: 4px; min-height: 24px; cursor: pointer; }
.total { margin-left: auto; color: var(--mute); font-variant-numeric: tabular-nums; }
.total.bad { color: var(--bad); }
/* The name and the numbers keep their widths; the bar takes the room that is left, with its share right-aligned after it. */
table { table-layout: fixed; }
.c-label { width: 30%; }
.c-n { width: 76px; }
.c-w { width: 100px; }
.bar { display: flex; align-items: center; gap: 8px; }
.track { flex: 1 1 auto; min-width: 40px; height: 10px; border-radius: 2px; background: #ececec; }
.fill { display: block; height: 100%; border-radius: 2px; background: #0072b2; }
.share { flex: none; width: 60px; text-align: right; }
@container (max-width: 899px) {
  .opt { display: none; }
}
</style>
