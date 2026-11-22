// The static server of the gate runs answers as the servers it replaces: `vite preview` for the
// UI (a page, else index.html) and `docusaurus serve` for the docs (base path, no trailing slash,
// `<path>/index.html` then `<path>.html`, 404.html), without loading a tool configuration first.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import type { AddressInfo } from 'node:net';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { serve } from './serve.ts';
import type { ServeOptions } from './serve.ts';

function site(files: Record<string, string>): string {
  // A file beside the served directory, which no request may reach.
  const root = mkdtempSync(join(tmpdir(), 'ux-serve-'));
  writeFileSync(join(root, 'secret.txt'), 'secret');
  const dir = join(root, 'site');
  mkdirSync(dir);
  for (const [path, body] of Object.entries(files)) {
    mkdirSync(join(dir, path, '..'), { recursive: true });
    writeFileSync(join(dir, path), body);
  }
  return dir;
}

async function answers(opts: ServeOptions, paths: string[]): Promise<string[]> {
  const server = await serve(opts, 0);
  const { port } = server.address() as AddressInfo;
  try {
    const out: string[] = [];
    for (const p of paths) {
      const r = await fetch(`http://127.0.0.1:${port}${p}`, { redirect: 'manual' });
      const body = r.status >= 300 && r.status < 400 ? `-> ${r.headers.get('location')}` : await r.text();
      out.push(`${p} ${r.status} ${r.headers.get('content-type')?.split(';')[0] ?? ''} ${body}`.trim());
    }
    return out;
  } finally {
    server.close();
  }
}

test('the docs are served as docusaurus serve serves them', async () => {
  const dir = site({
    'index.html': 'landing', '404.html': 'not found', 'docs.html': 'docs flat', 'docs/index.html': 'docs index',
    'docs/headroom.html': 'headroom', 'assets/app.js': 'js', 'search-index.json': '[]', 'img/a.svg': '<svg/>',
  });
  try {
    assert.deepEqual(await answers({ dir, base: '/slot/', mode: 'docs' }, [
      '/', '/other', '/slot/', '/slot/docs', '/slot/docs/', '/slot/docs/headroom', '/slot/docs/headroom/?q=1',
      '/slot/docs/headroom.html', '/slot/assets/app.js', '/slot/search-index.json', '/slot/img/a.svg',
      '/slot/docs/missing', '/slot/..%2fsecret.txt',
    ]), [
      '/ 302  -> /slot/',
      '/other 302  -> /slot/',
      '/slot/ 200 text/html landing',
      '/slot/docs 200 text/html docs index',
      '/slot/docs/ 302  -> /slot/docs',
      '/slot/docs/headroom 200 text/html headroom',
      '/slot/docs/headroom/?q=1 302  -> /slot/docs/headroom?q=1',
      '/slot/docs/headroom.html 301  -> /slot/docs/headroom',
      '/slot/assets/app.js 200 text/javascript js',
      '/slot/search-index.json 200 application/json []',
      '/slot/img/a.svg 200 image/svg+xml <svg/>',
      '/slot/docs/missing 404 text/html not found',
      '/slot/..%2fsecret.txt 404 text/html not found',
    ]);
  } finally {
    rmSync(join(dir, '..'), { recursive: true, force: true });
  }
});

test('the UI is served as vite preview serves it: a file, else index.html', async () => {
  const dir = site({ 'index.html': 'app', 'assets/index.js': 'js', 'assets/index.css': 'css' });
  try {
    assert.deepEqual(await answers({ dir, base: '/', mode: 'spa' }, ['/', '/jobs/42', '/assets/index.js', '/assets/index.css', '/assets/gone.js', '/..%2fsecret.txt']), [
      '/ 200 text/html app',
      '/jobs/42 200 text/html app',
      '/assets/index.js 200 text/javascript js',
      '/assets/index.css 200 text/css css',
      '/assets/gone.js 404 text/plain not found',
      '/..%2fsecret.txt 404 text/plain not found',
    ]);
  } finally {
    rmSync(join(dir, '..'), { recursive: true, force: true });
  }
});

test('the server started from the command line names its port and answers', async () => {
  const dir = site({ 'index.html': 'app' });
  const started = performance.now();
  const p = spawn(process.execPath, ['--disable-warning=ExperimentalWarning', fileURLToPath(new URL('./serve.ts', import.meta.url)), dir, '0'], {
    stdio: ['ignore', 'pipe', 'inherit'],
  });
  try {
    const port = await new Promise<number>((ok, fail) => {
      p.stdout.on('data', (b: Buffer) => {
        const m = /listening on http:\/\/127\.0\.0\.1:(\d+)\//.exec(String(b));
        if (m) ok(Number(m[1]));
      });
      p.on('exit', (code) => fail(new Error(`serve.ts exited ${code}`)));
    });
    assert.equal(await (await fetch(`http://127.0.0.1:${port}/`)).text(), 'app');
    // At load 160 it answered after 0.5 s, `docusaurus serve` after 4.7 s and `vite preview` after 2.8 s.
    assert.ok(performance.now() - started < 10_000, `started in ${Math.round(performance.now() - started)} ms`);
  } finally {
    p.kill();
    rmSync(join(dir, '..'), { recursive: true, force: true });
  }
});
