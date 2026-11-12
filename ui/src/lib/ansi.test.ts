import { test } from 'node:test';
import assert from 'node:assert/strict';
import { AnsiParser, PALETTE, PLAIN, Terminal, applySgr, color256, escapeHtml, lineHtml, lineIsErr } from './ansi.ts';
import type { Segment } from './ansi.ts';

const ESC = '\x1b';
const text = (line: readonly Segment[]) => line.map((s) => s.text).join('');

/** Feeds chunks into a fresh terminal; returns every completed line's text plus the current line's. */
function run(chunks: Array<string | [string, boolean]>): { lines: string[]; current: string; last: Segment[] } {
  const t = new Terminal();
  const lines: string[] = [];
  let cur: Segment[] = [];
  for (const c of chunks) {
    const [s, err] = typeof c === 'string' ? [c, false] : c;
    const r = t.write(s, err);
    lines.push(...r.done.map(text));
    cur = r.current;
  }
  return { lines, current: text(cur), last: cur };
}

test('plain text splits into lines', () => {
  assert.deepEqual(run(['a\nb', 'c\n']), { lines: ['a', 'bc'], current: '', last: [] });
});

test('SGR colour and bold become a styled segment, reset ends it', () => {
  const { last } = run([`${ESC}[1;32m   Compiling${ESC}[0m serde`]);
  assert.equal(last.length, 2);
  assert.deepEqual(last[0], { text: '   Compiling', style: { ...PLAIN, bold: true, fg: PALETTE[2] }, err: false });
  assert.deepEqual(last[1].style, PLAIN);
});

test('an escape sequence split across chunks is still parsed', () => {
  const { last, current } = run([`x${ESC}`, '[3', '1', `mred${ESC}[`, '0mok']);
  assert.equal(current, 'xredok');
  assert.equal(last[1].style.fg, PALETTE[1]);
  assert.deepEqual(last[2].style, PLAIN);
});

test('style carries over across chunks and lines', () => {
  const { lines, last } = run([`${ESC}[33mwarn\n`, 'still yellow']);
  assert.deepEqual(lines, ['warn']);
  assert.equal(last[0].style.fg, PALETTE[3]);
});

test('carriage return plus erase overwrites the progress line', () => {
  const chunks = ['    Building [=>  ] 1/300: a\r'];
  for (let k = 2; k <= 200; k++) chunks.push(`${ESC}[K    Building [==> ] ${k}/300: x\r`);
  chunks.push(`${ESC}[K   Compiling foo\n`);
  assert.deepEqual(run(chunks), { lines: ['   Compiling foo'], current: '', last: [] });
});

test('carriage return without erase still replaces the line on new text', () => {
  assert.equal(run(['progress 1\r', 'progress 2']).current, 'progress 2');
});

test('CRLF keeps the line', () => {
  assert.deepEqual(run(['abc\r\n']).lines, ['abc']);
  assert.deepEqual(run(['abc\r', '\n']).lines, ['abc']);
});

test('erase to end of line without a carriage return keeps the text; erase whole line clears it', () => {
  assert.equal(run([`abc${ESC}[K`]).current, 'abc');
  assert.equal(run([`abc${ESC}[2Kdef`]).current, 'def');
});

test('256-colour and truecolor foreground and background', () => {
  assert.equal(color256(1), PALETTE[1]);
  assert.equal(color256(16), '#000000');
  assert.equal(color256(196), '#ff0000');
  assert.equal(color256(231), '#ffffff');
  assert.equal(color256(232), '#080808');
  assert.equal(color256(255), '#eeeeee');
  assert.equal(color256(256), null);
  const s = applySgr(PLAIN, [38, 5, 208, 48, 2, 10, 20, 30]);
  assert.equal(s.fg, '#ff8700');
  assert.equal(s.bg, '#0a141e');
  assert.equal(run([`${ESC}[38:2:1:2:3mc`]).last[0].style.fg, '#010203');
});

test('bright colours, default resets and attribute offs', () => {
  let s = applySgr(PLAIN, [91, 104, 1, 2, 3, 4]);
  assert.deepEqual(s, { ...PLAIN, fg: PALETTE[9], bg: PALETTE[12], bold: true, dim: true, italic: true, underline: true });
  s = applySgr(s, [39, 49, 22, 23, 24]);
  assert.deepEqual(s, PLAIN);
});

test('malformed extended colours are ignored', () => {
  assert.deepEqual(applySgr(PLAIN, [38, 2, 300, 0, 0]), PLAIN);
  assert.deepEqual(applySgr(PLAIN, [38, 5]), PLAIN);
});

test('unknown CSI sequences and private modes are stripped', () => {
  assert.equal(run([`a${ESC}[2Ab${ESC}[?25lc${ESC}[10;20Hd${ESC}[?2004h`]).current, 'abcd');
});

test('OSC sequences are stripped, OSC 8 hyperlinks keep their text', () => {
  assert.equal(run([`see ${ESC}]8;;https://example.com/x${ESC}\\docs${ESC}]8;;${ESC}\\ ok`]).current, 'see docs ok');
  assert.equal(run([`${ESC}]0;title\x07after`]).current, 'after');
  assert.equal(run([`${ESC}]8;;https://ex`, `ample.com${ESC}`, '\\link']).current, 'link');
});

test('other control characters are dropped, tabs kept', () => {
  assert.equal(run(['a\x07b\x08c\td\x7f']).current, 'abc\td');
});

test('stdout and stderr keep separate escape state', () => {
  const t = new Terminal();
  t.write(`${ESC}[`, false);
  t.write(`${ESC}[31merr `, true);
  const r = t.write('3mout', false);
  assert.equal(text(r.current), 'err out');
  assert.equal(r.current[0].style.fg, PALETTE[1]);
  assert.equal(r.current[0].err, true);
  assert.deepEqual(r.current[1].style, { ...PLAIN, italic: true });
  assert.equal(r.current[1].err, false);
  assert.equal(lineIsErr(r.current), true);
});

test('a runaway CSI does not swallow the output forever', () => {
  const p = new AnsiParser();
  const ops = p.feed(`${ESC}[${'1;'.repeat(100)}visible`);
  assert.ok(ops.some((o) => o.k === 'text' && o.text.includes('visible')));
});

test('HTML is escaped in text and the style attribute holds only generated CSS', () => {
  assert.equal(escapeHtml(`<a href="x">&'`), '&lt;a href=&quot;x&quot;&gt;&amp;&#39;');
  const { last } = run([`${ESC}[1;31m<script>alert(1)</script>${ESC}[0m & "q"`]);
  assert.equal(
    lineHtml(last),
    `<span style="color:${PALETTE[1]};font-weight:600">&lt;script&gt;alert(1)&lt;/script&gt;</span> &amp; &quot;q&quot;`,
  );
});

test('inverse swaps colours with readable defaults', () => {
  assert.equal(lineHtml(run([`${ESC}[7mx`]).last), '<span style="color:#f6f6f6;background:#1a1a1a">x</span>');
});
