// Copy that goes stale with a release: the landing page's version and its status list.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

const site = join(dirname(fileURLToPath(import.meta.url)), '..');
const workspaceVersion = (): string => {
  const toml = readFileSync(join(site, '..', 'Cargo.toml'), 'utf8');
  const m = /\[workspace\.package\][^[]*?\nversion\s*=\s*"([^"]+)"/.exec(toml);
  assert.ok(m, 'no version in [workspace.package] of Cargo.toml');
  return m[1];
};

interface StatusItem {
  label: string;
  status: string;
  href: string;
}

/** Text of the section under the heading `#anchor` on a docs page, up to the next heading of the same level. */
function section(route: string, anchor: string): { heading: string; text: string } | undefined {
  const md = readFileSync(join(site, route.replace(/^\/docs\/?/, 'docs/') + '.md'), 'utf8').split('\n');
  const slugOf = (h: string): string => h.toLowerCase().replace(/`/g, '').replace(/[^a-z0-9 -]/g, '').trim().replace(/\s+/g, '-');
  const start = md.findIndex((l) => /^#{2,4} /.test(l) && slugOf(l.replace(/^#+ /, '')) === anchor);
  if (start < 0) return undefined;
  const level = /^#+/.exec(md[start])?.[0].length ?? 2;
  const end = md.findIndex((l, i) => i > start && new RegExp(`^#{2,${level}} `).test(l));
  return { heading: md[start].replace(/^#+ /, ''), text: md.slice(start + 1, end < 0 ? undefined : end).join('\n') };
}

test('every version on the landing page is the workspace version', () => {
  const v = workspaceVersion();
  const product = readFileSync(join(site, 'product.json'), 'utf8');
  for (const m of product.matchAll(/(?<![\d.])(\d+\.\d+\.\d+)(?![\d.])/g)) assert.equal(m[1], v, `product.json names ${m[1]}, the workspace is ${v}`);
});

test('a planned item on the status list points at a "Not built yet" section that still lists it', () => {
  const status = JSON.parse(readFileSync(join(site, 'data', 'status.json'), 'utf8')) as { items: StatusItem[] };
  for (const it of status.items) {
    assert.match(it.status, /^(shipped|planned)$/, `${it.label}: status ${it.status}`);
    if (it.status !== 'planned') continue;
    const [route, anchor] = it.href.split('#');
    const s = anchor ? section(route, anchor) : undefined;
    assert.ok(s, `${it.label}: ${it.href} is not a section of a docs page`);
    assert.match(s.heading, /not built/i, `${it.label}: planned, but ${it.href} is "${s.heading}"`);
    const word = it.label.toLowerCase().split(/\s+/).find((w) => w.length > 4) ?? it.label.toLowerCase();
    assert.ok(s.text.toLowerCase().includes(word), `${it.label}: planned, but "${s.heading}" on ${route} no longer mentions "${word}"`);
  }
});

test('the docs say "not built" or "planned" only under a "Not built yet" heading', () => {
  const pages = ['index', 'getting-started', 'how-it-works', 'toolchains', 'headroom', 'remote-builds', 'agent-integration', 'claude-code-plugin', 'reference/environment'];
  for (const p of pages) {
    const md = readFileSync(join(site, 'docs', `${p}.md`), 'utf8').split('\n');
    let heading = '';
    md.forEach((l, i) => {
      if (/^#{1,4} /.test(l)) heading = l;
      if (/\b(planned|not built|not implemented)\b/i.test(l) && !/not built/i.test(heading)) {
        assert.fail(`docs/${p}.md:${i + 1} says "${l.trim().slice(0, 70)}" outside a "Not built yet" section`);
      }
    });
  }
});
