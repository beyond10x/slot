<script setup lang="ts">
import { formatValue, themeValue } from '../../lib/spec-format.ts';
import type { Column, FieldGroup, Row } from '../../lib/spec.ts';
import SectionHead from '../SectionHead.vue';
import SpecHost from './SpecHost.vue';

/**
 * A `record` of the UI spec: one row as label and value, its fields first, then each group under
 * its label (a collapsed group opens on a click). A field without a value is left out.
 */
const props = defineProps<{
  path: string;
  title?: string;
  row: Row | undefined;
  status: 'loading' | 'ready' | 'failed';
  fields: Column[];
  groups: FieldGroup[];
}>();

const present = (cols: Column[]): Column[] =>
  props.row ? cols.filter((c) => c.host || formatValue(props.row?.[c.field], c) !== '') : [];
</script>

<template>
  <section class="spec-record" :data-ui-path="path">
    <SectionHead v-if="title" :text="title" />
    <p v-if="!row" class="muted">{{ status === 'loading' ? 'loading…' : status === 'failed' ? 'could not read this from the daemon' : 'the daemon holds no such record' }}</p>
    <template v-else>
      <dl class="uxui-dense">
        <template v-for="c in present(fields)" :key="c.field">
          <dt :class="['uxui-field-' + c.field, 'uxui-as-' + (c.as ?? 'text')]">{{ c.label }}</dt>
          <dd :class="['uxui-field-' + c.field, 'uxui-as-' + (c.as ?? 'text')]" :data-uxui-value="themeValue(row[c.field])">
            <SpecHost v-if="c.host" :name="c.host" :row="row" :params="{}" />
            <span v-else-if="c.as === 'badge'" class="uxui-badge">{{ formatValue(row[c.field], c) }}</span>
            <template v-else>{{ formatValue(row[c.field], c) }}</template>
          </dd>
        </template>
      </dl>
      <details v-for="g in groups" :key="g.name" :open="!g.collapsed">
        <summary>{{ g.label }}</summary>
        <dl class="uxui-dense">
          <template v-for="c in present(g.fields)" :key="c.field">
            <dt :class="['uxui-field-' + c.field, 'uxui-as-' + (c.as ?? 'text')]">{{ c.label }}</dt>
            <dd :class="['uxui-field-' + c.field, 'uxui-as-' + (c.as ?? 'text')]" :data-uxui-value="themeValue(row[c.field])">
              <SpecHost v-if="c.host" :name="c.host" :row="row" :params="{}" />
              <span v-else-if="c.as === 'badge'" class="uxui-badge">{{ formatValue(row[c.field], c) }}</span>
              <template v-else>{{ formatValue(row[c.field], c) }}</template>
            </dd>
          </template>
        </dl>
      </details>
    </template>
  </section>
</template>

<style scoped>
section { padding: 2px 14px 6px; min-width: 0; }
summary { cursor: pointer; font-size: 13px; font-weight: 700; line-height: 22px; }
</style>
