<script setup lang="ts">
import type { Job } from '../../generated/types.ts';
import type { Row } from '../../lib/spec.ts';
import RepoName from '../RepoName.vue';

/** Host slot: a job's repository, and its worktree in the quiet colour. */
defineProps<{ row?: Row; params: Record<string, string> }>();
</script>

<template>
  <span v-if="row" class="repo" :title="String(row.cwd ?? '')"><RepoName :job="row as unknown as Job" /></span>
</template>

<style scoped>
.repo .wt { color: var(--mute); }
</style>
