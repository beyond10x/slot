/**
 * Streaming ANSI parser and line model for build output.
 *
 * SGR (`ESC[...m`) becomes styled segments; every other CSI, OSC and
 * two-character escape is dropped (OSC 8 hyperlinks keep their text).
 * `\r` arms an overwrite: the next text or erase replaces the current line,
 * so a progress bar redrawn with `\r` + `ESC[K` shows only its latest state.
 * Parser state (style, partial escape) survives chunk boundaries.
 */

export interface Style {
  bold: boolean;
  dim: boolean;
  italic: boolean;
  underline: boolean;
  inverse: boolean;
  fg: string | null;
  bg: string | null;
}

export interface Segment {
  text: string;
  style: Style;
  err: boolean;
}

/** `erase` with `all` clears the whole line (`ESC[1K`, `ESC[2K`); without, only after a `\r` (cursor at column 0). */
export type Op = { k: 'text'; text: string; style: Style } | { k: 'nl' } | { k: 'cr' } | { k: 'erase'; all: boolean };

export const PLAIN: Style = Object.freeze({
  bold: false, dim: false, italic: false, underline: false, inverse: false, fg: null, bg: null,
});

/** 16-colour palette tuned for a light background (white and yellow darkened). */
export const PALETTE: readonly string[] = [
  '#1a1a1a', '#c4302b', '#2f8a2f', '#9a6700', '#1f5fbf', '#a336a3', '#1a7f8a', '#6e6e6e',
  '#555555', '#e0443e', '#3aa33a', '#b07d00', '#2f74d0', '#c24bc2', '#1f9aa6', '#4a4a4a',
];

const CUBE = [0, 95, 135, 175, 215, 255];
const hex2 = (n: number) => n.toString(16).padStart(2, '0');
const rgb = (r: number, g: number, b: number) => `#${hex2(r)}${hex2(g)}${hex2(b)}`;
const byte = (n: number | undefined): number | null =>
  n !== undefined && Number.isInteger(n) && n >= 0 && n <= 255 ? n : null;

/** xterm 256-colour index to a CSS colour. */
export function color256(n: number): string | null {
  if (byte(n) === null) return null;
  if (n < 16) return PALETTE[n];
  if (n < 232) {
    const k = n - 16;
    return rgb(CUBE[Math.floor(k / 36)], CUBE[Math.floor(k / 6) % 6], CUBE[k % 6]);
  }
  const g = 8 + (n - 232) * 10;
  return rgb(g, g, g);
}

/** Applies one SGR parameter list to a style. */
export function applySgr(style: Style, params: readonly number[]): Style {
  const s: Style = { ...style };
  const p = params.length ? params : [0];
  for (let k = 0; k < p.length; k++) {
    const c = p[k];
    if (c === 0) Object.assign(s, PLAIN);
    else if (c === 1) s.bold = true;
    else if (c === 2) s.dim = true;
    else if (c === 3) s.italic = true;
    else if (c === 4) s.underline = true;
    else if (c === 7) s.inverse = true;
    else if (c === 22) s.bold = s.dim = false;
    else if (c === 23) s.italic = false;
    else if (c === 24) s.underline = false;
    else if (c === 27) s.inverse = false;
    else if (c >= 30 && c <= 37) s.fg = PALETTE[c - 30];
    else if (c === 39) s.fg = null;
    else if (c >= 40 && c <= 47) s.bg = PALETTE[c - 40];
    else if (c === 49) s.bg = null;
    else if (c >= 90 && c <= 97) s.fg = PALETTE[c - 90 + 8];
    else if (c >= 100 && c <= 107) s.bg = PALETTE[c - 100 + 8];
    else if (c === 38 || c === 48) {
      let v: string | null = null;
      if (p[k + 1] === 5) {
        v = color256(p[k + 2] ?? -1);
        k += 2;
      } else if (p[k + 1] === 2) {
        const r = byte(p[k + 2]);
        const g = byte(p[k + 3]);
        const b = byte(p[k + 4]);
        v = r === null || g === null || b === null ? null : rgb(r, g, b);
        k += 4;
      }
      if (v !== null) {
        if (c === 38) s.fg = v;
        else s.bg = v;
      }
    }
  }
  return s;
}

const MAX_CSI = 64;
const MAX_OSC = 4096;

type Mode = 'text' | 'esc' | 'esc-charset' | 'csi' | 'osc' | 'osc-esc';

/** One stream's escape parser: turns text into line operations. */
export class AnsiParser {
  private mode: Mode = 'text';
  private seq = '';
  private oscLen = 0;
  private style: Style = PLAIN;

  feed(input: string): Op[] {
    const ops: Op[] = [];
    let text = '';
    const flush = () => {
      if (text) ops.push({ k: 'text', text, style: this.style });
      text = '';
    };
    for (const ch of input) {
      const code = ch.charCodeAt(0);
      switch (this.mode) {
        case 'text':
          if (ch === '\x1b') {
            flush();
            this.mode = 'esc';
          } else if (ch === '\n') {
            flush();
            ops.push({ k: 'nl' });
          } else if (ch === '\r') {
            flush();
            ops.push({ k: 'cr' });
          } else if (ch === '\t' || code >= 0x20) {
            if (code !== 0x7f) text += ch;
          }
          break;
        case 'esc':
          if (ch === '[') {
            this.mode = 'csi';
            this.seq = '';
          } else if (ch === ']') {
            this.mode = 'osc';
            this.oscLen = 0;
          } else if ('()*+-./#%'.includes(ch)) {
            this.mode = 'esc-charset';
          } else {
            this.mode = 'text';
          }
          break;
        case 'esc-charset':
          this.mode = 'text';
          break;
        case 'csi':
          if (code >= 0x40 && code <= 0x7e) {
            this.csi(this.seq, ch, ops);
            this.mode = 'text';
          } else if (code >= 0x20 && code <= 0x3f && this.seq.length < MAX_CSI) {
            this.seq += ch;
          } else {
            this.mode = ch === '\x1b' ? 'esc' : 'text';
          }
          break;
        case 'osc':
          if (ch === '\x07') this.mode = 'text';
          else if (ch === '\x1b') this.mode = 'osc-esc';
          else if (++this.oscLen > MAX_OSC) this.mode = 'text';
          break;
        case 'osc-esc':
          this.mode = ch === '\\' ? 'text' : 'osc';
          break;
      }
    }
    flush();
    return ops;
  }

  private csi(params: string, final: string, ops: Op[]): void {
    if (final === 'm' && /^[\d;:]*$/.test(params)) {
      const nums = params === '' ? [] : params.split(/[;:]/).map((x) => (x === '' ? 0 : Number(x)));
      this.style = applySgr(this.style, nums);
    } else if (final === 'K') {
      ops.push({ k: 'erase', all: params === '1' || params === '2' });
    }
  }
}

/**
 * Line model shared by stdout and stderr: each stream has its own parser,
 * both write into the same current line.
 */
export class Terminal {
  private readonly out = new AnsiParser();
  private readonly errp = new AnsiParser();
  private cur: Segment[] = [];
  private cr = false;

  /** Feeds a chunk; returns the lines it completed and the current (unfinished) line. */
  write(text: string, err: boolean): { done: Segment[][]; current: Segment[] } {
    const done: Segment[][] = [];
    for (const op of (err ? this.errp : this.out).feed(text)) {
      if (op.k === 'nl') {
        done.push(this.cur);
        this.cur = [];
        this.cr = false;
      } else if (op.k === 'cr') {
        this.cr = true;
      } else if (op.k === 'erase') {
        if (this.cr || op.all) this.cur = [];
      } else {
        if (this.cr) {
          this.cur = [];
          this.cr = false;
        }
        const last = this.cur[this.cur.length - 1];
        if (last && last.err === err && last.style === op.style) last.text += op.text;
        else this.cur.push({ text: op.text, style: op.style, err });
      }
    }
    return { done, current: this.cur };
  }
}

const ESC_HTML: Readonly<Record<string, string>> = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' };

export const escapeHtml = (s: string): string => s.replace(/[&<>"']/g, (c) => ESC_HTML[c]);

const INVERSE_FG = '#f6f6f6';
const INVERSE_BG = '#1a1a1a';

/** Inline CSS for a style; empty for plain text. Colours come only from PALETTE, color256 or rgb(). */
export function styleCss(s: Style): string {
  let fg = s.fg;
  let bg = s.bg;
  if (s.inverse) {
    fg = s.bg ?? INVERSE_FG;
    bg = s.fg ?? INVERSE_BG;
  }
  const css: string[] = [];
  if (fg) css.push(`color:${fg}`);
  if (bg) css.push(`background:${bg}`);
  if (s.bold) css.push('font-weight:600');
  if (s.dim) css.push('opacity:.65');
  if (s.italic) css.push('font-style:italic');
  if (s.underline) css.push('text-decoration:underline');
  return css.join(';');
}

/** HTML of one line, every text escaped. */
export function lineHtml(line: readonly Segment[]): string {
  return line
    .map((seg) => {
      const css = styleCss(seg.style);
      const text = escapeHtml(seg.text);
      return css ? `<span style="${css}">${text}</span>` : text;
    })
    .join('');
}

/** Whether any part of the line came from stderr. */
export const lineIsErr = (line: readonly Segment[]): boolean => line.some((s) => s.err);
