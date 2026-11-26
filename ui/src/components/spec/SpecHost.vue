<script setup lang="ts">
import type { Row } from '../../lib/spec.ts';
import type { HostName } from '../../generated/ui.ts';
import { hosts } from '../../hosts.ts';

/** A host slot of the UI spec: the hand-written component `src/hosts.ts` registers under its name. */
defineProps<{ name: HostName; row?: Row; params: Record<string, string> }>();
</script>

<template>
  <component :is="hosts[name]" :row="row" :params="params" />
</template>
