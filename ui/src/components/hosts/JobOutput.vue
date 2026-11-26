<script setup lang="ts">
import { computed } from 'vue';
import { jobId } from '../../lib/job.ts';
import type { Row } from '../../lib/spec.ts';
import { jobs } from '../../store.ts';
import OutputPane from '../OutputPane.vue';

/** Host slot: the job's output as the daemon streams it, in the output pane of the web view. */
const props = defineProps<{ row?: Row; params: Record<string, string> }>();

/**
 * The output pane's key for the job the page names by its id: the id for a job of this daemon;
 * for a row a node reported, `<node>/<id>`. This daemon's job wins, as `GET /api/jobs/{id}` answers.
 */
const key = computed(() => {
  const id = props.params.id;
  if (!id || jobs.has(id)) return id;
  for (const [k, j] of jobs) if (j.from_node && String(jobId(j)) === id) return k;
  return id;
});
</script>

<template>
  <OutputPane v-if="key" :id="key" :key="key" />
</template>
