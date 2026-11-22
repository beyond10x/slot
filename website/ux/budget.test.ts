// The test budget of the docs gates: Playwright stops a test at 60 s, every run ends with its ten
// slowest tests, and no test waits a fixed time.
// The 120 s deadline of the step is `ui/ux/deadline.ts`, held by `ui/ux/budget.test.ts`.
import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import config from '../playwright.config.ts';
import { pageLoads, RATCHET_VIEWPORT, VIEWPORTS, vpName } from './viewports.ts';
import type { Viewport } from './viewports.ts';

test('Playwright stops a docs test at 60 s', () => {
  assert.equal(config.timeout, 60_000);
});

test('every docs run ends with its ten slowest tests', () => {
  assert.deepEqual(config.reporter, [['line'], ['../ui/ux/slowest.ts']]);
});

// `docusaurus serve` loads the site configuration before it listens, and on a loaded host was not
// listening after Playwright's 60 s for a web server. A free port lets two worktrees run the gates at once.
test('the built site is served by the gate server, on a port of its own', () => {
  const web = config.webServer as { command: string; url: string };
  const port = process.env.UX_PORT;
  assert.match(port ?? '', /^\d+$/);
  assert.equal(web.command, `node --disable-warning=ExperimentalWarning ../ui/ux/serve.ts build ${port} --base /slot/ --docs`);
  assert.equal(web.url, `http://127.0.0.1:${port}/slot/`);
});

// A fixed wait is too short on a loaded host and too long on an idle one: `settle.ts` waits for the page.
// The reader's search and menu paths keep theirs (3): no task takes them, so a wait on a condition there was never run.
test('no docs test waits a fixed time', () => {
  const here = fileURLToPath(new URL('.', import.meta.url));
  const waits = readdirSync(here)
    .filter((f) => f.endsWith('.ts') && f !== 'budget.test.ts')
    .map((f): [string, number] => [f, readFileSync(join(here, f), 'utf8').split('waitForTimeout(').length - 1])
    .filter(([, n]) => n > 0);
  assert.deepEqual(waits, [['bot.ts', 3]]);
});

// A page load with its hydration and axe-core's load is most of a gate test's time: the gating
// sizes share one load per page and theme, the ratchet size first, so its layout shift is that of
// the load. A size only a round measures keeps a load of its own.
test('the gates load a page once per theme for both gating sizes, and measure every size once', () => {
  const names = (loads: Viewport[][]): string[][] => loads.map((l) => l.map(vpName));
  const gating = VIEWPORTS.filter((v) => v.gating).map(vpName);
  assert.equal(gating.length, 2);
  assert.equal(gating[0], RATCHET_VIEWPORT);
  assert.deepEqual(names(pageLoads(false)), [gating]);
  const round = names(pageLoads(true));
  assert.deepEqual(round[0], gating);
  assert.deepEqual(round.slice(1), VIEWPORTS.filter((v) => !v.gating).map((v) => [vpName(v)]));
  assert.deepEqual(round.flat().sort(), VIEWPORTS.map(vpName).sort());
  const gates = readFileSync(fileURLToPath(new URL('./gates.spec.ts', import.meta.url)), 'utf8');
  assert.match(gates, /for \(const sizes of pageLoads\(\)\)/);
  assert.doesNotMatch(gates, /viewportsInRun\(/);
});
