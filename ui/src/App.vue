<script setup lang="ts">
import { onMounted, onUnmounted, ref, watchEffect } from 'vue';
import ClusterView from './components/ClusterView.vue';
import JobsView from './JobsView.vue';

/** The view of the address: `#jobs` the job tables, charts and output; anything else the cluster start page. */
const viewOf = (hash: string): 'jobs' | 'cluster' => (hash === '#jobs' ? 'jobs' : 'cluster');
const view = ref(viewOf(location.hash));
const onHash = (): void => {
  view.value = viewOf(location.hash);
};
onMounted(() => window.addEventListener('hashchange', onHash));
onUnmounted(() => window.removeEventListener('hashchange', onHash));

// The cluster page has a dark and a light theme; the job tables are light only.
const theme = ref<'dark' | 'light'>(localStorage.getItem('slot-theme') === 'light' ? 'light' : 'dark');
const setTheme = (t: 'dark' | 'light'): void => {
  theme.value = t;
  localStorage.setItem('slot-theme', t);
};
watchEffect(() => {
  document.documentElement.dataset.theme = view.value === 'cluster' ? theme.value : 'light';
});
</script>

<template>
  <ClusterView v-if="view === 'cluster'" :theme="theme" @theme="setTheme" />
  <JobsView v-else />
</template>
