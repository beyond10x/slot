<script setup lang="ts">
import { computed } from 'vue';
import type { Job } from '../../generated/types.ts';
import { fmtDur } from '../../lib/format.ts';
import { waitS } from '../../lib/job.ts';
import type { Row } from '../../lib/spec.ts';
import { store } from '../../store.ts';

/** Host slot: how long a job waited for its slots, up to its start, or to now while it waits. */
const props = defineProps<{ row?: Row; params: Record<string, string> }>();
const text = computed(() => (props.row ? fmtDur(waitS(props.row as unknown as Job, store.clock)) : ''));
</script>

<template>
  <span class="num">{{ text }}</span>
</template>
