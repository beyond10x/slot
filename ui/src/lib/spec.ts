/**
 * The runtime of the pages `slot-docs ui-pages` generates from the UI spec into src/generated/: hash
 * routes, page state in the URL, view reads on the routes of spec/http.yaml, and the operators of a
 * read's filter. Hand-written; the generated pages import it.
 */
import { onUnmounted, reactive, ref, watch } from 'vue';
import { routes, views, type HostName, type PageName, type ViewName } from '../generated/ui.ts';

/** A row of a view, as the daemon sends it. */
export type Row = Record<string, unknown>;

/** One field of a table or a record, as the UI spec declares it. */
export interface Column {
  field: string;
  label: string;
  as?: 'text' | 'number' | 'badge' | 'list' | 'map' | 'time' | 'timestamp' | 'duration' | 'bytes';
  unit?: 'ms' | 's' | 'm' | 'h';
  tones?: Readonly<Record<string, string>>;
  host?: HostName;
}

/** Fields of a record shown under one heading. */
export interface FieldGroup {
  name: string;
  label: string;
  collapsed: boolean;
  fields: Column[];
}

/** A filter bar: a search and choices, each writing one key of the page state. */
export interface FilterBar {
  search?: { key: string; placeholder: string };
  choices: Array<{ key: string; label: string; placeholder: string; options: string[] }>;
}

// ---------------------------------------------------------------------------------------------
// Routes: `#/<segments>?<page state>`.

/** The page the hash names, with its params and the page state in its query. */
export interface Route {
  page: PageName;
  params: Record<string, string>;
  query: URLSearchParams;
}

/** Reads a hash: the page it names, or null when it names none (the dashboard, at no hash or `#/`). */
export function parseHash(hash: string): Route | null {
  if (!hash.startsWith('#/')) return null;
  const [path = '', search = ''] = hash.slice(2).split('?', 2);
  const segs = path.split('/').filter((s) => s !== '');
  for (const [page, pattern] of Object.entries(routes) as Array<[PageName, readonly string[]]>) {
    if (pattern.length !== segs.length) continue;
    const params: Record<string, string> = {};
    const hit = pattern.every((p, i) => {
      const s = decodeURIComponent(segs[i]!);
      if (p.startsWith(':')) {
        params[p.slice(1)] = s;
        return true;
      }
      return p === s;
    });
    if (hit) return { page, params, query: new URLSearchParams(search) };
  }
  return null;
}

/** The hash of a page with its params. */
export function href(page: PageName, params: Record<string, string>): string {
  const segs = routes[page].map((p) => (p.startsWith(':') ? encodeURIComponent(params[p.slice(1)] ?? '') : p));
  return `#/${segs.join('/')}`;
}

/** The part of the browser's window the routes use; absent under node, where the tests import this file. */
interface Browser {
  location: { hash: string };
  history: { replaceState(data: unknown, unused: string, url: string): void };
  addEventListener(type: 'hashchange', listener: () => void): void;
}
const browser = (globalThis as { window?: Browser }).window;

const hash = ref(browser?.location.hash ?? '');
browser?.addEventListener('hashchange', () => {
  hash.value = browser.location.hash;
});

/** The page the location names now, or null for the dashboard. */
export function currentRoute(): Route | null {
  return parseHash(hash.value);
}

/** Opens a page. */
export function go(page: PageName, params: Record<string, string>): void {
  if (browser) browser.location.hash = href(page, params);
}

/** Page state held in the query of the hash, one string per key; a link reproduces it. */
export function useUrlState(keys: readonly string[]): Record<string, string> {
  const query = parseHash(hash.value)?.query ?? new URLSearchParams();
  const state = reactive(Object.fromEntries(keys.map((k) => [k, query.get(k) ?? '']))) as Record<string, string>;
  watch(state, () => {
    if (!browser) return;
    const [path = ''] = browser.location.hash.split('?', 1);
    const q = new URLSearchParams();
    for (const k of keys) if (state[k]) q.set(k, state[k]);
    const next = q.size ? `${path}?${q}` : path;
    browser.history.replaceState(null, '', next || '#/');
    hash.value = browser.location.hash;
  });
  return state;
}

// ---------------------------------------------------------------------------------------------
// Reads.

/** A read of one view: its rows and where it is. */
export interface Read {
  rows: Row[];
  status: 'loading' | 'ready' | 'failed';
  error: string;
}

/** Reads of one URL within this many ms share one request. */
const SHARE_MS = 500;
const inflight = new Map<string, { at: number; answer: Promise<Row[]> }>();

/** The URL of a view read: its path with each path variable filled from its param. */
export function viewUrl(view: ViewName, params: Record<string, string>): string {
  const v = views[view];
  return v.path.replace(/\{([^}]+)\}/g, (_, name: string) => {
    const param = (v.params as Record<string, string>)[name] ?? name;
    return encodeURIComponent(params[param] ?? '');
  });
}

async function fetchRows(view: ViewName, url: string): Promise<Row[]> {
  const r = await fetch(url);
  const body = views[view].body;
  if (body === 'single' && r.status === 404) return [];
  if (!r.ok) throw new Error(`${r.status} ${view}`);
  const json: unknown = await r.json();
  if (body === 'single') return [json as Row];
  if (body === 'array') return json as Row[];
  return (json as { rows: Row[] }).rows;
}

function shared(view: ViewName, url: string): Promise<Row[]> {
  const now = Date.now();
  const held = inflight.get(url);
  if (held && now - held.at < SHARE_MS) return held.answer;
  const answer = fetchRows(view, url);
  inflight.set(url, { at: now, answer });
  return answer;
}

/** Reads `view` with `params`, again every `refreshMs` (0: once) and whenever the params change. */
export function useView(view: ViewName, params: () => Record<string, string>, refreshMs: number): Read {
  const read = reactive<Read>({ rows: [], status: 'loading', error: '' });
  let seq = 0;
  const load = async (): Promise<void> => {
    const mine = ++seq;
    try {
      const rows = await shared(view, viewUrl(view, params()));
      if (mine !== seq) return;
      read.rows = rows;
      read.status = 'ready';
      read.error = '';
    } catch (e) {
      if (mine !== seq) return;
      read.status = 'failed';
      read.error = e instanceof Error ? e.message : String(e);
    }
  };
  watch(params, () => void load(), { immediate: true, deep: true });
  if (refreshMs > 0) {
    const timer = setInterval(() => void load(), refreshMs);
    onUnmounted(() => clearInterval(timer));
  }
  onUnmounted(() => {
    seq++;
  });
  return read;
}

// ---------------------------------------------------------------------------------------------
// Filter operators: the meaning `ess-ui/1` gives `==`, `in` and `contains`.

/** A value as text: a list is its items joined by spaces. */
export function text(v: unknown): string {
  if (v === undefined || v === null) return '';
  if (Array.isArray(v)) return v.map(text).join(' ');
  if (typeof v === 'object') return JSON.stringify(v);
  return String(v);
}

/** `a == b`. */
export function eq(a: unknown, b: unknown): boolean {
  return text(a) === text(b);
}

/** `a in [..]`. */
export function among(a: unknown, list: unknown[]): boolean {
  return list.some((b) => eq(a, b));
}

/** `a contains b`: `a` holds every word of `b`, in any case; true while `b` has none. */
export function contains(a: unknown, b: unknown): boolean {
  const words = text(b).toLowerCase().split(/\s+/).filter((w) => w !== '');
  const hay = text(a).toLowerCase();
  return words.every((w) => hay.includes(w));
}
