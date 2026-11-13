<script setup lang="ts">
import { onMounted, onUnmounted, ref, watch } from 'vue';
import type { By, Usage } from '../generated/types.ts';
import { usageRows, usageUrl } from '../lib/usage.ts';
import type { UsageLine, UsageWindow } from '../lib/usage.ts';

const windows: [UsageWindow, string][] = [['today', 'today'], ['24h', 'last 24 h'], ['7d', 'last 7 days']];
const groups: [By, string][] = [['project', 'project'], ['repo', 'repository'], ['session', 'session']];

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
    <span class="muted">{{ failed || total }}</span>
  </div>
  <table>
    <thead>
      <tr>
        <th>{{ by }}</th><th>share of slot time</th><th class="num">slot-h</th><th class="num">CPU share</th>
        <th class="num">jobs</th><th class="num opt">median wait</th>
      </tr>
    </thead>
    <tbody>
      <tr v-for="r in rows" :key="r.key" class="static">
        <td class="label" :title="r.key">{{ r.label }}</td>
        <td class="bar"><span class="track"><span class="fill" :style="{ width: `${r.width}%` }"></span></span> {{ r.share }}</td>
        <td class="num">{{ r.slotHours }}</td>
        <td class="num">{{ r.cpuShare }}</td>
        <td class="num">{{ r.jobs }}</td>
        <td class="num opt">{{ r.wait }}</td>
      </tr>
      <tr v-if="!rows.length && !failed" class="empty"><td colspan="6" class="muted">no finished jobs in this window</td></tr>
    </tbody>
  </table>
</template>

<style scoped>
.controls { display: flex; flex-wrap: wrap; gap: 4px 16px; align-items: center; margin: 4px 0 6px; }
fieldset { border: 0; margin: 0; padding: 0; display: flex; gap: 10px; }
.sr { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0 0 0 0); }
tr.static { cursor: default; }
td.label { max-width: 260px; overflow: hidden; text-overflow: ellipsis; }
td.bar { white-space: nowrap; }
.track { display: inline-block; width: 140px; height: 10px; background: #f6f6f6; vertical-align: middle; margin-right: 6px; }
.fill { display: block; height: 100%; background: #0072B2; }
</style>
