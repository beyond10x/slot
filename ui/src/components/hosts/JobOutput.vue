<script setup lang="ts">
import type { Row } from '../../lib/spec.ts';
import OutputPane from '../OutputPane.vue';

/** Host slot: the job's output as the daemon streams it, in the output pane of the web view. */
defineProps<{ row?: Row; params: Record<string, string> }>();
</script>

<template>
  <OutputPane v-if="params.id" :id="Number(params.id)" :key="params.id" />
</template>
