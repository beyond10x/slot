<script setup lang="ts">
import { tcColor } from '../lib/colors.ts';

defineProps<{ name: string }>();
</script>

<template>
  <span class="tool"><span class="dot" :style="{ background: tcColor(name) }"></span><span class="tn">{{ name }}</span></span>
</template>
