<script setup lang="ts">
import { tcColor } from '../lib/colors.ts';

defineProps<{ name: string }>();
</script>

<template>
  <span><span class="dot" :style="{ background: tcColor(name) }"></span>{{ name }}</span>
</template>
