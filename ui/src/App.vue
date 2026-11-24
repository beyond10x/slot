<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watchEffect } from 'vue';
import ClusterView from './components/ClusterView.vue';
import HeaderBar from './components/HeaderBar.vue';
import SpecRoute from './components/spec/SpecRoute.vue';
import JobsView from './JobsView.vue';
import { currentRoute } from './lib/spec.ts';

/** The view of the address: `#jobs` the job tables, charts and output; anything else the cluster start page. */
const viewOf = (hash: string): 'jobs' | 'cluster' => (hash === '#jobs' ? 'jobs' : 'cluster');
const view = ref(viewOf(location.hash));
const onHash = (): void => {
  view.value = viewOf(location.hash);
};
onMounted(() => window.addEventListener('hashchange', onHash));
onUnmounted(() => window.removeEventListener('hashchange', onHash));

/** The generated page of the UI spec the hash names (`#/jobs`, `#/jobs/<id>`); null shows a view above. */
const specRoute = computed(() => currentRoute());

// The cluster page has a dark and a light theme; the job tables and the generated pages are light only.
const theme = ref<'dark' | 'light'>(localStorage.getItem('slot-theme') === 'light' ? 'light' : 'dark');
const setTheme = (t: 'dark' | 'light'): void => {
  theme.value = t;
  localStorage.setItem('slot-theme', t);
};
watchEffect(() => {
  document.documentElement.dataset.theme = view.value === 'cluster' && !specRoute.value ? theme.value : 'light';
});
</script>

<template>
  <template v-if="specRoute">
    <HeaderBar />
    <SpecRoute :route="specRoute" />
  </template>
  <ClusterView v-else-if="view === 'cluster'" :theme="theme" @theme="setTheme" />
  <JobsView v-else />
</template>
