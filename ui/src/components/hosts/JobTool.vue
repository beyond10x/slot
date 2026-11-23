<script setup lang="ts">
import type { Row } from '../../lib/spec.ts';
import ToolDot from '../ToolDot.vue';

/** Host slot: a job's toolchain with the colour every chart gives it. */
defineProps<{ row?: Row; params: Record<string, string> }>();
</script>

<template>
  <span class="c-tool" :title="String(row?.toolchain ?? '')"><ToolDot :name="String(row?.toolchain ?? '')" /></span>
</template>
