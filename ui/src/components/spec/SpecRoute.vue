<script setup lang="ts">
import { pageComponents } from '../../generated/pages.ts';
import type { Route } from '../../lib/spec.ts';

/** The page of the UI spec the hash names (`#/jobs`, `#/jobs/42`), generated into src/generated/pages/. */
defineProps<{ route: Route }>();
</script>

<template>
  <component :is="pageComponents[route.page]" :key="`${route.page}/${JSON.stringify(route.params)}`" :params="route.params" />
</template>
