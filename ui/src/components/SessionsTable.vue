<script setup lang="ts">
import { computed } from 'vue';
import { fmtBytes } from '../lib/format.ts';
import { notOnSlot, orderSessions, sessionCells } from '../lib/host.ts';
import { census, sessionSparks, store } from '../store.ts';
import CoresSpark from './CoresSpark.vue';

const rows = computed(() =>
  orderSessions(census.value?.sessions ?? []).map((s) => ({
    key: s.session,
    cells: sessionCells(s),
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
        <th>session</th><th class="c-id">id</th><th class="c-n num">cores</th><th class="c-w num">not on slot</th><th class="c-n num">procs</th>
        <th class="c-m num opt">RSS</th><th class="c-s opt">last 60 s</th><th class="c-b"><span class="sr">flag</span></th>
      </tr>
    </thead>
    <tbody>
      <tr v-for="r in rows" :key="r.key" class="static" :class="{ flag: r.flag }">
        <td class="label" :class="{ untitled: r.cells.untitled }" :title="r.id ? `${r.cells.name}\nsession ${r.id}` : 'processes without a session id'">{{ r.cells.name }}</td>
        <td class="id" :title="r.id || undefined">{{ r.cells.id }}</td>
        <td class="num">{{ r.cores }}</td>
        <td class="num">{{ r.unmanaged }}</td>
        <td class="num">{{ r.procs }}</td>
        <td class="num opt">{{ r.rss }}</td>
        <td class="opt"><CoresSpark :points="r.spark" :t1="store.clock" :span="sessionSparks.spanMs" /></td>
        <td><span v-if="r.flag" class="badge">not on slot</span></td>
      </tr>
      <tr v-if="!rows.length" class="empty"><td colspan="8">no sessions seen yet</td></tr>
    </tbody>
  </table>
</template>

<style scoped>
/* The session's name takes the room that is left of a table no wider than 1200 px, so its numbers stay close enough to read across. */
table { table-layout: fixed; width: min(100% + 12px, 1200px); }
.c-id { width: 96px; }
.c-n { width: 70px; }
.c-w { width: 96px; }
.c-m { width: 84px; }
.c-s { width: 96px; }
.c-b { width: 104px; }
/* A session without a title: the words are quiet, its id stands in the id column like every other. */
.untitled { color: var(--mute); }
.badge { display: inline-block; padding: 0 8px; border-radius: 10px; background: var(--warn); color: #fff; font-size: 12px; font-weight: 600; line-height: 20px; }
@container (max-width: 899px) {
  .opt { display: none; }
}
</style>
