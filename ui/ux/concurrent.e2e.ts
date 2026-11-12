// Two gate runs at the same time, as two worktrees running `task check` do: each must get its own
// static server and pass. Needs ui/dist; run by `task ui-ux` after the gates.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { dirname, join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

const ui = join(dirname(fileURLToPath(import.meta.url)), '..');

function gateRun(): Promise<{ code: number | null; out: string }> {
  const env = { ...process.env };
  delete env.UX_PORT;
  delete env.UX_ALL;
  const p = spawn('pnpm', ['exec', 'playwright', 'test', '--workers=1', '--grep', 'layout idle @ 1366x768'], {
    cwd: ui,
    env,
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  let out = '';
  p.stdout.on('data', (b: Buffer) => (out += b));
  p.stderr.on('data', (b: Buffer) => (out += b));
  return new Promise((done) => p.on('close', (code) => done({ code, out })));
}

test('two gate runs at the same time both pass', { timeout: 240_000 }, async () => {
  const runs = await Promise.all([gateRun(), gateRun()]);
  for (const [i, r] of runs.entries()) assert.equal(r.code, 0, `run ${i + 1} failed:\n${r.out}`);
});
