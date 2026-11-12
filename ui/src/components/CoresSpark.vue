<script setup lang="ts">
import { computed } from 'vue';
import { sparkPath } from '../lib/spark.ts';
import type { Point } from '../lib/spark.ts';

const W = 72;
const H = 16;

const props = defineProps<{ points: readonly Point[]; t1: number; span: number }>();

const vmax = computed(() => Math.max(1, ...props.points.map((p) => p.v)));
const d = computed(() => sparkPath(props.points, props.t1, props.span, W, H, vmax.value));
</script>

<template>
  <svg :width="W" :height="H + 2" :viewBox="`0 -1 ${W} ${H + 2}`" class="spark" role="img"
    :aria-label="`cores over the last ${span / 1000} s, peak ${vmax.toFixed(1)}`">
    <title>cores, last {{ span / 1000 }} s (peak {{ vmax.toFixed(1) }})</title>
    <line x1="0" :y1="H" :x2="W" :y2="H" class="base" />
    <path :d="d" class="line" />
  </svg>
</template>

<style scoped>
.spark { vertical-align: middle; }
.base { stroke: #ddd; stroke-width: 1; }
.line { fill: none; stroke: #222; stroke-width: 1.2; }
</style>
