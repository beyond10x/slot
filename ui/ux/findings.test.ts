// No S1 or S2 finding closes without a regression asset that exists.
import assert from 'node:assert/strict';
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const dir = join(here, 'findings');

/** Header fields: `key: value` lines before the first blank line after the title. */
export function header(md: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const line of md.split('\n').slice(1)) {
    if (!line.trim()) {
      if (Object.keys(out).length) break;
      continue;
    }
    const m = /^-?\s*([a-z_]+):\s*(.*)$/.exec(line.trim());
    if (m) out[m[1]] = m[2].trim();
  }
  return out;
}

const files = existsSync(dir) ? readdirSync(dir).filter((f) => /^UXF-\d{3}\.md$/.test(f)) : [];

test('every finding has the header fields', () => {
  for (const f of files) {
    const h = header(readFileSync(join(dir, f), 'utf8'));
    for (const k of ['id', 'title', 'class', 'severity', 'metric', 'status', 'found_by', 'regression_asset']) {
      assert.ok(k in h, `${f}: no ${k}`);
    }
    assert.equal(h.id, f.replace('.md', ''), `${f}: id`);
    assert.match(h.severity, /^S[123]$/, `${f}: severity`);
  }
});

test('a closed S1 or S2 finding names a regression asset that exists', () => {
  for (const f of files) {
    const h = header(readFileSync(join(dir, f), 'utf8'));
    if (h.status !== 'closed' || h.severity === 'S3') continue;
    const asset = h.regression_asset ?? '';
    assert.ok(asset && asset !== 'none', `${f}: closed ${h.severity} without a regression asset`);
    const path = asset.split(/\s+/)[0].replace(/^`|`$/g, '');
    assert.ok(existsSync(join(here, '..', path)), `${f}: regression asset ${path} does not exist`);
  }
});
