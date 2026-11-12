<script setup lang="ts">
import { computed } from 'vue';
import type { LiveJob } from '../generated/types.ts';
import { fmtDur } from '../lib/format.ts';
import { kindStyle, orderUnits, unitText, unitsDone } from '../lib/units.ts';

const props = defineProps<{ live: LiveJob | undefined }>();

const chips = computed(() =>
  orderUnits(props.live?.units ?? []).map((u, k) => ({
    key: `${u.kind}:${u.name}:${k}`,
    style: kindStyle(u.kind),
    text: unitText(u, fmtDur),
  })),
);
const done = computed(() => (props.live ? unitsDone(props.live) : 0));
</script>

<template>
  <div class="units">
    <span v-for="c in chips" :key="c.key" class="chip" :style="{ borderColor: c.style.color }">
      <b :style="{ background: c.style.color }">{{ c.style.label }}</b>{{ c.text }}
    </span>
    <span v-if="!chips.length" class="muted">no units sampled yet</span>
    <span class="muted">{{ done }} units done so far</span>
  </div>
</template>

<style scoped>
.units { display: flex; flex-wrap: wrap; gap: 4px 6px; align-items: center; padding: 2px 0 4px 22px; white-space: normal; }
.chip { display: inline-flex; align-items: center; gap: 5px; border: 1px solid; border-radius: 3px; padding: 0 6px 0 0;
  font: 12px/18px ui-monospace, monospace; background: #fff; }
.chip b { color: #fff; font-weight: 600; padding: 0 5px; border-radius: 2px 0 0 2px; font-family: system-ui, sans-serif; font-size: 11px; }
</style>
