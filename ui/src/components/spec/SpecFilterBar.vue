<script setup lang="ts">
import type { FilterBar } from '../../lib/spec.ts';

/** A `filter_bar` of the UI spec: a search and choices, each writing one key of the page state. */
defineProps<{ path: string; bar: FilterBar; state: Record<string, string> }>();
</script>

<template>
  <div class="filters" :data-ui-path="path">
    <label v-if="bar.search" class="search">
      <span class="sr">search</span>
      <input v-model="state[bar.search.key]" type="search" :placeholder="bar.search.placeholder" />
    </label>
    <label v-for="c in bar.choices" :key="c.key">
      <span class="sr">{{ c.label }}</span>
      <select v-model="state[c.key]">
        <option value="">{{ c.placeholder || c.label }}</option>
        <option v-for="o in c.options" :key="o" :value="o">{{ o }}</option>
      </select>
    </label>
  </div>
</template>

<style scoped>
.filters { display: flex; flex-wrap: wrap; align-items: center; gap: 6px 10px; }
.search input { min-height: 24px; width: 220px; padding: 0 6px; border: 1px solid var(--ctl); border-radius: 4px; font-size: 13px; }
</style>
