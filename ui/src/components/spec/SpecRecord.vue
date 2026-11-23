<script setup lang="ts">
import { formatValue, toneOf } from '../../lib/spec-format.ts';
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
      <dl>
        <template v-for="c in present(fields)" :key="c.field">
          <dt>{{ c.label }}</dt>
          <dd :class="'as-' + (c.as ?? 'text')">
            <SpecHost v-if="c.host" :name="c.host" :row="row" :params="{}" />
            <span v-else :class="c.tones ? 'tone-' + toneOf(row[c.field], c) : undefined">{{ formatValue(row[c.field], c) }}</span>
          </dd>
        </template>
      </dl>
      <details v-for="g in groups" :key="g.name" :open="!g.collapsed">
        <summary>{{ g.label }}</summary>
        <dl>
          <template v-for="c in present(g.fields)" :key="c.field">
            <dt>{{ c.label }}</dt>
            <dd :class="'as-' + (c.as ?? 'text')">
              <SpecHost v-if="c.host" :name="c.host" :row="row" :params="{}" />
              <span v-else :class="c.tones ? 'tone-' + toneOf(row[c.field], c) : undefined">{{ formatValue(row[c.field], c) }}</span>
            </dd>
          </template>
        </dl>
      </details>
    </template>
  </section>
</template>

<style scoped>
section { padding: 2px 14px 6px; min-width: 0; }
dl { display: grid; grid-template-columns: max-content minmax(0, 1fr); gap: 1px 14px; margin: 4px 0 8px; font-size: 13px; line-height: 20px;
  font-variant-numeric: tabular-nums; }
dt { color: var(--mute); }
dd { margin: 0; overflow-wrap: anywhere; }
dd.as-list, dd.as-map { font-family: var(--mono); white-space: pre-wrap; }
summary { cursor: pointer; font-size: 13px; font-weight: 700; line-height: 22px; }
.tone-danger { color: var(--bad); font-weight: 600; }
.tone-warning { color: var(--warn); }
.tone-success { color: #1e7a3a; }
.tone-info { color: var(--accent); }
</style>
