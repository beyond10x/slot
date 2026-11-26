<script setup lang="ts">
import { computed } from 'vue';
import type { Job } from '../../generated/types.ts';
import { fmtDur } from '../../lib/format.ts';
import { runS } from '../../lib/job.ts';
import type { Row } from '../../lib/spec.ts';
import { store } from '../../store.ts';

/** Host slot: how long a job ran, up to its end, or to now while it runs; empty if it never started. */
const props = defineProps<{ row?: Row; params: Record<string, string> }>();
const text = computed(() => {
  const s = props.row ? runS(props.row as unknown as Job, store.clock) : null;
  return s === null ? '' : fmtDur(s);
});
</script>

<template>
  <span class="num">{{ text }}</span>
</template>
