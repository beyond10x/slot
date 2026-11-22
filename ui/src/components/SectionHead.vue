<script setup lang="ts">
import { computed } from 'vue';

/** A section heading "Name: what it shows": the name stands out, the rest reads as its caption. The text is one string, unchanged. */
const props = defineProps<{ text: string }>();

const parts = computed(() => {
  const k = props.text.indexOf(':');
  return k < 0 ? { name: props.text, rest: '' } : { name: props.text.slice(0, k), rest: props.text.slice(k) };
});
</script>

<template>
  <h2><span class="name">{{ parts.name }}</span>{{ parts.rest }}</h2>
</template>
