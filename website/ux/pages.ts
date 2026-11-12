// The page list: every route in the built site's sitemap, relative to the site base.
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export const BASE_PATH = '/slot/';
const here = dirname(fileURLToPath(import.meta.url));

export function sitemapPages(): string[] {
  const xml = readFileSync(resolve(here, '..', process.env.UX_BUILD ?? 'build', 'sitemap.xml'), 'utf8');
  const routes = [...xml.matchAll(/<loc>([^<]+)<\/loc>/g)].map((m) => new URL(m[1]).pathname);
  return [...new Set(routes.map((p) => p.replace(BASE_PATH, '').replace(/\/$/, '')))].sort();
}

/** A file-name-safe id for a route: `''` is the landing page. */
export const slug = (route: string): string => (route === '' ? 'landing' : route.replace(/\//g, '_'));

export const isDocPage = (route: string): boolean => route.startsWith('docs');
