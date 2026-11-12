<script setup lang="ts">
import { computed } from 'vue';
import { fmtBytes } from '../lib/format.ts';
import { notOnSlot, orderSessions, sessionLabel } from '../lib/host.ts';
import { census, sessionSparks, store } from '../store.ts';
import CoresSpark from './CoresSpark.vue';

const rows = computed(() =>
  orderSessions(census.value?.sessions ?? []).map((s) => ({
    key: s.session,
    label: sessionLabel(s),
    id: s.session,
    cores: s.cores.toFixed(1),
    unmanaged: s.unmanaged_cores.toFixed(1),
    procs: s.procs,
    rss: fmtBytes(s.rss),
    flag: notOnSlot(s),
    spark: [...sessionSparks.get(s.session)],
  })),
);
</script>

<template>
  <table>
    <thead>
      <tr>
        <th>session</th><th class="num">cores</th><th class="num">not on slot</th><th class="num">procs</th>
        <th class="num">RSS</th><th>last 60 s</th><th></th>
      </tr>
    </thead>
    <tbody>
      <tr v-for="r in rows" :key="r.key" class="static" :class="{ flag: r.flag }">
        <td class="label" :title="r.id || 'processes without a session id'">{{ r.label }}</td>
        <td class="num">{{ r.cores }}</td>
        <td class="num">{{ r.unmanaged }}</td>
        <td class="num">{{ r.procs }}</td>
        <td class="num">{{ r.rss }}</td>
        <td><CoresSpark :points="r.spark" :t1="store.clock" :span="sessionSparks.spanMs" /></td>
        <td><span v-if="r.flag" class="badge" title="builds outside slot use more than 1 core">not on slot</span></td>
      </tr>
      <tr v-if="!rows.length" class="empty"><td colspan="7" class="muted">no sessions seen yet</td></tr>
    </tbody>
  </table>
</template>

<style scoped>
tr.static { cursor: default; }
td.label { max-width: 360px; overflow: hidden; text-overflow: ellipsis; }
tr.flag td { background: #fdf0e6; }
.badge { font-size: 12px; padding: 0 6px; border-radius: 3px; background: #D55E00; color: #fff; font-weight: 600; }
</style>
