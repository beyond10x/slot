<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { formatValue, isNumeric, themeValue } from '../../lib/spec-format.ts';
import type { Column, Row } from '../../lib/spec.ts';
import SectionHead from '../SectionHead.vue';
import SpecHost from './SpecHost.vue';

/**
 * A `collection` of the UI spec: its rows as a table in the look of the job tables. The row's key
 * opens it (its row action); a field with a host slot draws through the host's component.
 */
const props = defineProps<{
  path: string;
  title?: string;
  columns: Column[];
  rows: Row[];
  status: 'loading' | 'ready' | 'failed';
  rowKey: string;
  empty?: string;
  sizes?: number[];
  openLabel?: string;
}>();
const emit = defineEmits<{ open: [row: Row] }>();

const size = ref(props.sizes?.[0] ?? 0);
const page = ref(1);
const pages = computed(() => (size.value ? Math.max(1, Math.ceil(props.rows.length / size.value)) : 1));
watch(pages, (n) => {
  if (page.value > n) page.value = n;
});
const shown = computed(() => (size.value ? props.rows.slice((page.value - 1) * size.value, page.value * size.value) : props.rows));
const range = computed(() => {
  if (!size.value || !props.rows.length) return '';
  const from = (page.value - 1) * size.value + 1;
  return `${from}-${Math.min(props.rows.length, page.value * size.value)} of ${props.rows.length}`;
});
const heading = computed(() => (props.title ? `${props.title}: ${props.rows.length}` : ''));
const emptyText = computed(() =>
  props.status === 'failed' ? 'could not read this from the daemon' : props.status === 'loading' ? 'loading…' : (props.empty ?? 'nothing'),
);
</script>

<template>
  <section class="spec-table uxui-dense-frame" :data-ui-path="path">
    <SectionHead v-if="heading" :text="heading" />
    <table class="jobs spec uxui-dense">
      <thead>
        <tr>
          <th v-for="c in columns" :key="c.field + c.label" :class="['uxui-field-' + c.field, 'uxui-as-' + (c.as ?? 'text'), { num: isNumeric(c) }]">{{ c.label }}</th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="row in shown" :key="String(row[rowKey])" @click="emit('open', row)">
          <td v-for="c in columns" :key="c.field + c.label" :class="['uxui-field-' + c.field, 'uxui-as-' + (c.as ?? 'text'), { num: isNumeric(c) }]"
            :data-uxui-value="themeValue(row[c.field])" :title="c.as === 'list' ? formatValue(row[c.field], c) : undefined">
            <SpecHost v-if="c.host" :name="c.host" :row="row" :params="{}" />
            <button v-else-if="c.field === rowKey && openLabel" type="button" class="jobid" :aria-label="`${openLabel} ${String(row[rowKey])}`"
              @click.stop="emit('open', row)">{{ formatValue(row[c.field], c) }}</button>
            <span v-else-if="c.as === 'badge'" class="uxui-badge">{{ formatValue(row[c.field], c) }}</span>
            <template v-else>{{ formatValue(row[c.field], c) }}</template>
          </td>
        </tr>
        <tr v-if="!shown.length" class="empty uxui-empty"><td :colspan="columns.length">{{ emptyText }}</td></tr>
      </tbody>
    </table>
    <p v-if="sizes && rows.length > (sizes[0] ?? 0)" class="pager">
      <button type="button" class="btn" :disabled="page <= 1" @click="page--">previous</button>
      <span>{{ range }}</span>
      <button type="button" class="btn" :disabled="page >= pages" @click="page++">next</button>
      <label>per page <select v-model.number="size" @change="page = 1"><option v-for="s in sizes" :key="s" :value="s">{{ s }}</option></select></label>
    </p>
  </section>
</template>

<style scoped>
/* Column widths, one-line cells and the colour per value come from the uxui themes (src/uxui/). */
section { padding: 2px 14px 6px; }
.pager { display: flex; align-items: center; gap: 10px; margin: 6px 0 0; font-size: 13px; color: var(--mute); font-variant-numeric: tabular-nums; }
.pager label { display: inline-flex; align-items: center; gap: 4px; margin-left: auto; }
</style>
