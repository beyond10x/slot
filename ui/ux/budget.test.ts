// The test budget of the gates: the runner stops a test at 60 s and names the slowest tests, and a step stops at 120 s.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import config from '../playwright.config.ts';
import { report, slowest } from './slowest.ts';

test('Playwright stops a test at 60 s and reports the ten slowest over 5 s', () => {
  assert.equal(config.timeout, 60_000);
  assert.deepEqual(config.reportSlowTests, { max: 10, threshold: 5_000 });
});

// reportSlowTests names only files that ran in one worker, so a parallel run prints nothing: the gates list tests themselves.
test('every Playwright run ends with its ten slowest tests, slowest first', () => {
  assert.deepEqual(config.reporter, [['line'], ['./ux/slowest.ts']]);
  const runs = Array.from({ length: 12 }, (_, i) => ({ title: `t${i}`, ms: i * 1000 }));
  const lines = slowest(runs);
  assert.equal(lines.length, 10);
  assert.equal(lines[0], '  11.0 s  t11');
  assert.equal(lines[9], '  2.0 s  t2');
  assert.deepEqual(slowest([{ title: 'only', ms: 450 }]), ['  0.5 s  only']);
});

// `vite preview` loads Vite and its plugins before it listens; the gate's own server only reads files.
test('the built UI is served by the gate server, on the port of the run', () => {
  const web = config.webServer as { command: string; url: string };
  assert.equal(web.command, `node --disable-warning=ExperimentalWarning ux/serve.ts dist ${process.env.UX_PORT}`);
  assert.equal(web.url, `http://127.0.0.1:${process.env.UX_PORT}/`);
});

test('a run about to be stopped names the tests it is in, longest first', () => {
  assert.deepEqual(report([{ title: 'a', ms: 1200 }]), ['Slowest tests:', '  1.2 s  a']);
  assert.deepEqual(report([{ title: 'a', ms: 1200 }], [{ title: 'b', ms: 300 }, { title: 'c', ms: 41_000 }]), [
    'Slowest tests:', '  1.2 s  a', 'Still running:', '  41.0 s  c', '  0.3 s  b',
  ]);
});

/** The lines of a task in Taskfile.yml, up to the next task. */
function task(taskfile: string, name: string): string[] {
  const lines = taskfile.split('\n');
  const start = lines.indexOf(`  ${name}:`);
  assert.notEqual(start, -1, `task ${name}`);
  const end = lines.findIndex((l, i) => i > start && /^  \S/.test(l));
  return lines.slice(start + 1, end === -1 ? undefined : end).map((l) => l.trim()).filter((l) => l !== '');
}

// The test commands of both steps run under one deadline, each started as itself, not through npx
// or pnpm: at load 175 `npx playwright --version` took 6.8 s and the binary 2.5 s. The docs typecheck
// is a compile, not a test (9.3 s to 26.9 s there): it runs before the deadline, as the UI's runs in `ui-check`.
test('both UX steps run all their test commands under one deadline of 120 s', () => {
  const taskfile = readFileSync(fileURLToPath(new URL('../../Taskfile.yml', import.meta.url)), 'utf8');
  const ui = task(taskfile, 'ui-ux');
  assert.deepEqual(ui.slice(ui.indexOf('cmds:') + 1), [
    '- |',
    "node ux/deadline.ts 120 sh -exc '",
    './node_modules/.bin/playwright test',
    'node ux/ratchet.ts',
    "node --test --disable-warning=ExperimentalWarning ux/concurrent.e2e.ts'",
  ]);
  const site = task(taskfile, 'site-ux');
  assert.deepEqual(site.slice(site.indexOf('cmds:') + 1), [
    '- ./node_modules/.bin/tsc --noEmit -p ux/tsconfig.json',
    '- |',
    "node ../ui/ux/deadline.ts 120 sh -exc '",
    'node --test --disable-warning=ExperimentalWarning "ux/*.test.ts"',
    './node_modules/.bin/playwright test',
    "node ux/ratchet.ts'",
  ]);
  const gate = task(taskfile, 'check');
  assert.ok(gate.includes('- task: ui-ux') && gate.includes('- task: site-ux'));
});

test('the test of two gate runs at once is stopped at 60 s like every other', () => {
  const e2e = readFileSync(fileURLToPath(new URL('./concurrent.e2e.ts', import.meta.url)), 'utf8');
  assert.match(e2e, /\{ timeout: 60_000 \}/);
});
