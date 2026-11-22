// The test budget of the docs gates: Playwright stops a test at 60 s, every run ends with its ten
// slowest tests, and no test waits a fixed time.
// The 120 s deadline of the step is `ui/ux/deadline.ts`, held by `ui/ux/budget.test.ts`.
import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import config from '../playwright.config.ts';

test('Playwright stops a docs test at 60 s', () => {
  assert.equal(config.timeout, 60_000);
});

test('every docs run ends with its ten slowest tests', () => {
  assert.deepEqual(config.reporter, [['line'], ['../ui/ux/slowest.ts']]);
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
