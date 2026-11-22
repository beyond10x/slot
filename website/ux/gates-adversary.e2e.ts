// adversary u2
// Gate strength at 1280x720 since one page load per theme measures both gating sizes: the docs gates
// run against a fixture site, from a copy of website/ux so the worktree's ux/out keeps its records.
//   landing: a media query widens the page below 1300 px; a resize applies it, so the gate fails at 1280x720.
// Not caught any more: a page that a script lays out wider only when it is first drawn at 1280x720, as
// 1280x720 is measured after a resize from 1366x768. One load per theme is worth that: the site lays
// itself out by the window size only at 996 px (Docusaurus), below both gating sizes.
// Needs Playwright's Chromium; run: node --test --disable-warning=ExperimentalWarning ux/gates-adversary.e2e.ts
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { copyFileSync, mkdirSync, mkdtempSync, readdirSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { after, before, test } from 'node:test';
import { fileURLToPath } from 'node:url';

const website = join(dirname(fileURLToPath(import.meta.url)), '..');
const repo = join(website, '..');

const page = (head: string): string => `<!doctype html>
<html lang="en" data-has-hydrated="true">
<head><meta charset="utf-8"><title>Fixture</title>
<style>
body { margin: 0; color: #000; background: #fff; }
nav.navbar { position: fixed; top: 0; left: 0; right: 0; height: 50px; display: flex; gap: 8px; align-items: center; background: #fff; }
main { padding-top: 60px; }
.wide { width: 100%; height: 20px; }
</style>
${head}
</head>
<body>
<nav class="navbar" aria-label="Main"><input type="search" aria-label="Search"><button type="button" title="Switch between dark and light mode">Theme</button></nav>
<main><article><h1>Fixture</h1><p>Text of the page.</p><div class="wide"></div></article></main>
</body>
</html>
`;

interface Outcome {
  title: string;
  status: string;
  message: string;
}

let root = '';
let outcomes: Outcome[] = [];
let raw = '';

before(() => {
  root = mkdtempSync(join(tmpdir(), 'ux-gates-adv-'));
  const site = join(root, 'website');
  mkdirSync(join(site, 'ux'), { recursive: true });
  mkdirSync(join(root, 'ui', 'ux'), { recursive: true });
  for (const f of readdirSync(join(website, 'ux')).filter((f) => f.endsWith('.ts') || f === 'package.json')) {
    copyFileSync(join(website, 'ux', f), join(site, 'ux', f));
  }
  for (const f of readdirSync(join(repo, 'ui', 'ux')).filter((f) => f.endsWith('.ts'))) {
    copyFileSync(join(repo, 'ui', 'ux', f), join(root, 'ui', 'ux', f));
  }
  writeFileSync(join(root, 'ui', 'package.json'), '{ "type": "module" }\n');
  copyFileSync(join(website, 'playwright.config.ts'), join(site, 'playwright.config.ts'));
  symlinkSync(join(website, 'node_modules'), join(site, 'node_modules'));
  const fixture = join(site, 'fixture');
  mkdirSync(fixture);
  writeFileSync(
    join(fixture, 'sitemap.xml'),
    '<?xml version="1.0"?><urlset><url><loc>https://example.org/slot/</loc></url></urlset>\n',
  );
  writeFileSync(join(fixture, 'index.html'), page('<style>@media (max-width: 1300px) { .wide { width: 1400px; } }</style>'));
  writeFileSync(join(fixture, '404.html'), page(''));
  const env: NodeJS.ProcessEnv = { ...process.env, UX_BUILD: 'fixture' };
  delete env.UX_PORT;
  delete env.UX_ALL;
  const run = spawnSync(join(website, 'node_modules', '.bin', 'playwright'), ['test', 'ux/gates.spec.ts', '--reporter=json', '--workers=2'], {
    cwd: site,
    env,
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
  });
  raw = `${run.stdout}\n${run.stderr}`;
  type Suite = { suites?: Suite[]; specs?: Array<{ title: string; tests: Array<{ status: string; results: Array<{ error?: { message?: string } }> }> }> };
  const walk = (s: Suite): void => {
    for (const spec of s.specs ?? []) {
      for (const t of spec.tests) {
        outcomes.push({ title: spec.title, status: t.status, message: t.results.map((r) => r.error?.message ?? '').join('\n') });
      }
    }
    for (const c of s.suites ?? []) walk(c);
  };
  try {
    walk(JSON.parse(run.stdout) as Suite);
  } catch {
    outcomes = [];
  }
});

after(() => {
  if (root) rmSync(root, { recursive: true, force: true });
});

const of = (slug: string): Outcome[] => outcomes.filter((o) => o.title.startsWith(`${slug} @ `));

test('adversary u2: a page that scrolls sideways at 1280x720 by a media query fails the docs gates in both themes', { timeout: 60_000 }, () => {
  const landing = of('landing');
  assert.equal(landing.length, 2, raw.slice(0, 4000));
  for (const o of landing) {
    assert.equal(o.status, 'unexpected', `${o.title}: ${o.status}`);
    assert.match(o.message, /1280x720: page scrolls sideways/, o.title);
    assert.doesNotMatch(o.message, /1366x768: page scrolls sideways/, o.title);
  }
});

