<script setup lang="ts">
import type { Job } from '../generated/types.ts';
import { checkoutLabel } from '../lib/job.ts';

/** A job's checkout on one line: the repository, and the worktree's name after it in the quiet colour. */
defineProps<{ job: Job }>();
</script>

<template>
  <template v-if="job.checkout?.kind === 'worktree'">{{ job.repo }} <span class="wt">worktree {{ job.checkout.name }}</span></template>
  <template v-else>{{ checkoutLabel(job) }}</template>
</template>
