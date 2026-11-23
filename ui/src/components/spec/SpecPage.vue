<script setup lang="ts">
import { computed } from 'vue';

/**
 * A page of the UI spec: its header (title, badge, filters, links) and its sections, stacked or
 * in the columns of its layout (`narrow` and `wide`).
 */
const props = defineProps<{
  path: string;
  title: string;
  badge?: string;
  links?: Array<{ label: string; href: string }>;
  columns?: Array<'narrow' | 'wide'>;
}>();

const grid = computed(() =>
  (props.columns ?? []).map((w) => (w === 'wide' ? 'minmax(0, 3fr)' : 'minmax(300px, 2fr)')).join(' '),
);
</script>

<template>
  <div class="spec-page" :data-ui-path="path">
    <div class="head">
      <h1>{{ title }}</h1>
      <span v-if="badge" class="state">{{ badge }}</span>
      <slot name="filters" />
      <nav v-if="links?.length" class="links">
        <a v-for="l in links" :key="l.href" :href="l.href">{{ l.label }}</a>
      </nav>
    </div>
    <div v-if="columns" class="cols" :style="{ gridTemplateColumns: grid }">
      <div v-for="(_, i) in columns" :key="i" class="col"><slot :name="`col${i}`" /></div>
    </div>
    <slot v-else />
  </div>
</template>

<style scoped>
.head { display: flex; flex-wrap: wrap; align-items: center; gap: 6px 14px; padding: 10px 14px 4px; }
h1 { margin: 0; font-size: 16px; line-height: 24px; font-variant-numeric: tabular-nums; }
.state { padding: 0 7px; border-radius: 10px; background: #ececec; font-size: 13px; line-height: 20px; }
.links { margin-left: auto; font-size: 13px; }
.links a { color: var(--accent); }
.cols { display: grid; align-items: start; }
.col { min-width: 0; }
@media (max-width: 1099px) {
  .cols { grid-template-columns: minmax(0, 1fr) !important; }
}
</style>
