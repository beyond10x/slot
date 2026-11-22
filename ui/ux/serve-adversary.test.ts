// adversary u2
// Cases where the gate server answers otherwise than the servers it replaced: `vite preview` stops
// at once on a build directory that does not exist, and `docusaurus serve` (serve-handler,
// `symlinks: false`) answers 404 for a symlink, so no request reaches a file outside the build.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import type { AddressInfo } from 'node:net';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { serve } from './serve.ts';

test('adversary u2: the server started on a build directory that does not exist exits non-zero at once, as vite preview does', async () => {
  const root = mkdtempSync(join(tmpdir(), 'ux-serve-adv-'));
  const missing = join(root, 'dist');
  const p = spawn(process.execPath, ['--disable-warning=ExperimentalWarning', fileURLToPath(new URL('./serve.ts', import.meta.url)), missing, '0'], {
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  try {
    const outcome = await new Promise<string>((ok) => {
      p.stdout.on('data', (b: Buffer) => {
        if (/listening on/.test(String(b))) ok(`listening: ${String(b).trim()}`);
      });
      p.on('exit', (code) => ok(`exit ${code}`));
      setTimeout(() => ok('still running after 5 s'), 5_000).unref();
    });
    // Playwright waits for a status from 200 to 403: a server that answers every path with 404 or
    // 500 holds the run for the 60 s of `webServer.timeout` and then names no cause.
    assert.match(outcome, /^exit [1-9]/, `a server on a missing build directory: ${outcome}`);
  } finally {
    p.kill();
    rmSync(root, { recursive: true, force: true });
  }
});

test('adversary u2: docs mode answers 404 for a symlink that leads out of the build directory, as docusaurus serve does', async () => {
  const root = mkdtempSync(join(tmpdir(), 'ux-serve-adv-'));
  writeFileSync(join(root, 'secret.txt'), 'secret');
  const dir = join(root, 'site');
  mkdirSync(dir);
  writeFileSync(join(dir, 'index.html'), 'landing');
  writeFileSync(join(dir, '404.html'), 'not found');
  symlinkSync(join(root, 'secret.txt'), join(dir, 'leak.txt'));
  symlinkSync(root, join(dir, 'up'));
  const server = await serve({ dir, base: '/slot/', mode: 'docs' }, 0);
  const { port } = server.address() as AddressInfo;
  try {
    const got: string[] = [];
    for (const p of ['/slot/leak.txt', '/slot/up/secret.txt']) {
      const r = await fetch(`http://127.0.0.1:${port}${p}`, { redirect: 'manual' });
      got.push(`${p} ${r.status} ${await r.text()}`);
    }
    assert.deepEqual(got, ['/slot/leak.txt 404 not found', '/slot/up/secret.txt 404 not found']);
  } finally {
    server.close();
    rmSync(root, { recursive: true, force: true });
  }
});

test('adversary u2: /slot/index.html leads to the address of the landing page, /slot/', async () => {
  const root = mkdtempSync(join(tmpdir(), 'ux-serve-adv-'));
  const dir = join(root, 'site');
  mkdirSync(dir);
  writeFileSync(join(dir, 'index.html'), 'landing');
  writeFileSync(join(dir, '404.html'), 'not found');
  const server = await serve({ dir, base: '/slot/', mode: 'docs' }, 0);
  const { port } = server.address() as AddressInfo;
  try {
    // Follow the redirects by hand: the address the browser ends on is what the site's router reads.
    let path = '/slot/index.html';
    const hops: string[] = [path];
    for (let i = 0; i < 4; i++) {
      const r = await fetch(`http://127.0.0.1:${port}${path}`, { redirect: 'manual' });
      const to = r.headers.get('location');
      if (r.status < 300 || r.status >= 400 || !to) break;
      path = to;
      hops.push(path);
    }
    assert.equal(path, '/slot/', `hops: ${hops.join(' -> ')}`);
  } finally {
    server.close();
    rmSync(root, { recursive: true, force: true });
  }
});
