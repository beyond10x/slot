<script setup lang="ts">
import { computed } from 'vue';
import type { Job } from '../generated/types.ts';
import { idLabel, jobName, remoteNode } from '../lib/job.ts';

/** The job cell of the job tables: the id opens the output; a job of a build node names that node on a line below. */
const props = defineProps<{ job: Job }>();
const emit = defineEmits<{ open: [e: MouseEvent] }>();
const node = computed(() => remoteNode(props.job));
</script>

<template>
  <td class="jid">
    <button type="button" class="jobid" :aria-label="`output of job ${jobName(job)}`" @click.stop="emit('open', $event)">{{ idLabel(job) }}</button>
    <span v-if="node" class="where" :title="`on build node ${node}`">{{ node }}</span>
  </td>
</template>
