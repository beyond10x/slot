<script setup lang="ts">
import SectionHead from '../SectionHead.vue';

/** A section of a page of the UI spec that is not a table: its heading and what it holds. */
defineProps<{ path: string; title?: string }>();
</script>

<template>
  <section class="spec-section" :data-ui-path="path">
    <SectionHead v-if="title" :text="title" />
    <slot />
  </section>
</template>

<style scoped>
section { padding: 2px 14px 6px; min-width: 0; }
</style>
