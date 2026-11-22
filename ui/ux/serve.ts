// The static server of a gate run: node ux/serve.ts <dir> <port> [--base /slot/ --docs]
// Serves built files the way the server it replaces does: `vite preview` for the UI (`spa`: a file,
// else index.html) and `docusaurus serve` for the docs (`docs`: under the base path, without a
// trailing slash, `<path>/index.html` then `<path>.html`, else 404.html). Both of those load their
// tool's configuration and plugins before they listen: on a loaded host `docusaurus serve` was not
// listening after 60 s, Playwright's limit for a web server. This one only reads files.
import { realpathSync, statSync } from 'node:fs';
import { lstat, readFile, realpath } from 'node:fs/promises';
import { createServer } from 'node:http';
import type { IncomingMessage, Server, ServerResponse } from 'node:http';
import { extname, join, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

export interface ServeOptions {
  /** The built files. */
  dir: string;
  /** Path every page is under, with a slash at both ends. */
  base: string;
  mode: 'spa' | 'docs';
}

const TYPES: Record<string, string> = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.map': 'application/json; charset=utf-8',
  '.xml': 'application/xml; charset=utf-8',
  '.txt': 'text/plain; charset=utf-8',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.jpeg': 'image/jpeg',
  '.webp': 'image/webp',
  '.gif': 'image/gif',
  '.ico': 'image/x-icon',
  '.woff': 'font/woff',
  '.woff2': 'font/woff2',
  '.ttf': 'font/ttf',
  '.wasm': 'application/wasm',
};

/**
 * The file at `rel` under `dir` (a real path); undefined when there is none, when it is a symlink
 * (`docusaurus serve` answers those 404), or when `rel` or a linked directory leads out of `dir`.
 */
async function file(dir: string, rel: string): Promise<string | undefined> {
  const inside = (p: string): boolean => p === dir || p.startsWith(dir + sep);
  const path = resolve(dir, `.${sep}${rel}`);
  if (!inside(path)) return undefined;
  try {
    const st = await lstat(path);
    if (!st.isFile()) return undefined;
    return inside(await realpath(path)) ? path : undefined;
  } catch {
    return undefined;
  }
}

async function send(res: ServerResponse, path: string, status = 200): Promise<void> {
  const body = await readFile(path);
  res.writeHead(status, { 'Content-Type': TYPES[extname(path).toLowerCase()] ?? 'application/octet-stream', 'Content-Length': body.length });
  res.end(body);
}

function redirect(res: ServerResponse, status: number, location: string): void {
  res.writeHead(status, { Location: location });
  res.end();
}

async function notFound(res: ServerResponse, dir: string, mode: ServeOptions['mode']): Promise<void> {
  const page = mode === 'docs' ? await file(dir, '404.html') : undefined;
  if (page) return send(res, page, 404);
  res.writeHead(404, { 'Content-Type': 'text/plain; charset=utf-8' });
  res.end('not found');
}

/** Answers one request as `docusaurus serve` (trailingSlash: false) or `vite preview` does. */
export function handler(opts: ServeOptions): (req: IncomingMessage, res: ServerResponse) => Promise<void> {
  let dir = resolve(opts.dir);
  try {
    dir = realpathSync(dir);
  } catch {
    /* missing: every request is 404 */
  }
  return async (req, res) => {
    const url = new URL(req.url ?? '/', 'http://localhost');
    const path = url.pathname;
    if (!path.startsWith(opts.base)) return redirect(res, 302, opts.base);
    let rel: string;
    try {
      rel = decodeURIComponent(path.slice(opts.base.length));
    } catch {
      return notFound(res, dir, opts.mode);
    }
    if (opts.mode === 'spa') {
      const found = await file(dir, rel === '' ? 'index.html' : rel);
      if (found) return send(res, found);
      // vite preview gives index.html for a page's path, and 404 for a missing asset.
      if (extname(rel) !== '') return notFound(res, dir, opts.mode);
      return send(res, join(dir, 'index.html'));
    }
    const asset = /\.[a-zA-Z\d]{1,4}$/.test(path);
    if (!asset && path !== opts.base && path.endsWith('/')) return redirect(res, 302, `${path.slice(0, -1)}${url.search}`);
    // `x.html` and `x/index` lead to `x`, both at once: `index.html` leads to the base itself.
    if (/(\.html|\/index)$/.test(`/${rel}`)) {
      let clean = `/${rel}`;
      for (let next = clean.replace(/(\.html|\/index)$/, ''); next !== clean; next = clean.replace(/(\.html|\/index)$/, '')) clean = next;
      return redirect(res, 301, `${opts.base}${clean.slice(1)}`);
    }
    const candidates = extname(rel) !== '' ? [rel] : [];
    candidates.push(join(rel, 'index.html'), ...(rel === '' ? [] : [`${rel.replace(/\/$/, '')}.html`]), rel);
    for (const c of candidates) {
      const found = await file(dir, c);
      if (found) return send(res, found);
    }
    return notFound(res, dir, opts.mode);
  };
}

/** Listens on 127.0.0.1:`port` (0: a free port). */
export function serve(opts: ServeOptions, port: number): Promise<Server> {
  const handle = handler(opts);
  const server = createServer((req, res) => {
    handle(req, res).catch(() => {
      if (!res.headersSent) res.writeHead(500);
      res.end();
    });
  });
  return new Promise((ok, fail) => {
    server.once('error', fail);
    server.listen(port, '127.0.0.1', () => ok(server));
  });
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [dir, port, ...rest] = process.argv.slice(2);
  const at = rest.indexOf('--base');
  const base = at >= 0 ? rest[at + 1] : '/';
  if (!dir || !(Number(port) >= 0) || !base?.startsWith('/') || !base.endsWith('/')) {
    console.error('usage: node ux/serve.ts <dir> <port> [--base /path/] [--docs]');
    process.exit(2);
  }
  const mode = rest.includes('--docs') ? 'docs' : 'spa';
  // Playwright waits 60 s for a server that answers 404 everywhere, then names no cause.
  let isDir = false;
  try {
    isDir = statSync(dir).isDirectory();
  } catch {
    /* missing */
  }
  if (!isDir) {
    console.error(`serve: ${resolve(dir)} is not a directory; build it first with \`${mode === 'docs' ? 'task site' : 'task ui'}\``);
    process.exit(1);
  }
  const server = await serve({ dir, base, mode }, Number(port));
  const { port: bound } = server.address() as { port: number };
  console.log(`serve: ${dir} listening on http://127.0.0.1:${bound}${base}`);
  for (const s of ['SIGINT', 'SIGTERM'] as const) process.on(s, () => process.exit(0));
}
