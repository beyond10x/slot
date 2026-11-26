/**
 * How the component set draws a value by the `as` of its field in the UI spec. Times are the
 * local wall clock, as everywhere in the web view; a list reads as its items joined by spaces,
 * like a command line.
 */
import { fmtBytes, fmtClock, fmtDur } from './format.ts';
import type { Column } from './spec.ts';

const UNIT_S: Record<NonNullable<Column['unit']>, number> = { ms: 0.001, s: 1, m: 60, h: 3600 };

/** A value as text for a field: empty for an absent value and for a time of 0 (not set). */
export function formatValue(value: unknown, col: Pick<Column, 'as' | 'unit'>): string {
  if (value === undefined || value === null || value === '') return '';
  switch (col.as) {
    case 'number': {
      const n = Number(value);
      return Number.isInteger(n) ? String(n) : n.toFixed(1);
    }
    case 'bytes':
      return fmtBytes(Number(value));
    case 'time':
      return Number(value) > 0 ? fmtClock(Number(value)) : '';
    case 'timestamp': {
      const ms = Number(value);
      if (!(ms > 0)) return '';
      return `${new Date(ms).toLocaleDateString('sv-SE')} ${fmtClock(ms)}`;
    }
    case 'duration':
      return fmtDur(Number(value) * UNIT_S[col.unit ?? 'ms']);
    case 'list':
      return Array.isArray(value) ? value.map(String).join(' ') : String(value);
    case 'map':
      return typeof value === 'object'
        ? Object.entries(value as Record<string, unknown>)
            .map(([k, v]) => `${k}=${typeof v === 'object' ? JSON.stringify(v) : String(v)}`)
            .join('\n')
        : String(value);
    default:
      return typeof value === 'object' ? JSON.stringify(value) : String(value);
  }
}

/** The tone of a value by its field's tone map; neutral when the map does not name it. */
export function toneOf(value: unknown, col: Pick<Column, 'tones'>): string {
  return col.tones?.[String(value)] ?? 'neutral';
}

/** A value as the theme's `data-uxui-value`: a scalar as text; nothing for a list, a map or no value. */
export function themeValue(value: unknown): string | undefined {
  if (value === '' || value === null || value === undefined) return undefined;
  return typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean' ? String(value) : undefined;
}

/** Fields drawn right-aligned. */
export const isNumeric = (col: Column): boolean =>
  col.as === 'number' || col.as === 'bytes' || col.as === 'duration';
