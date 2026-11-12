/** Formats seconds as `4.2s`, `42s`, `3m07`, `1.5h`. Negative and NaN read as 0. */
export function fmtDur(s: number): string {
  if (!(s > 0)) s = 0;
  if (s < 100) return s.toFixed(s < 10 ? 1 : 0) + 's';
  if (s < 6000) return Math.floor(s / 60) + 'm' + String(Math.floor(s % 60)).padStart(2, '0');
  return (s / 3600).toFixed(1) + 'h';
}

const MIB = 1 << 20;
const GIB = 1 << 30;

/** Formats bytes as whole MB below 1 GiB, else GB with one decimal; 0 is empty. */
export function fmtBytes(b: number): string {
  if (!b) return '';
  if (b < GIB) return (b / MIB).toFixed(0) + ' MB';
  return (b / GIB).toFixed(1) + ' GB';
}

const STEPS_S = [10, 30, 60, 120, 300, 600, 1800, 3600];

/** Axis tick spacing in milliseconds for a span of seconds: at most about 10 ticks. */
export function axisStepMs(spanS: number): number {
  const s = STEPS_S.find((x) => spanS / x <= 10) ?? 3600 * Math.ceil(spanS / 36000);
  return s * 1000;
}

/** Tick times (ms) on multiples of `stepMs` within [t0, t1]. */
export function axisTicks(t0: number, t1: number, stepMs: number): number[] {
  const out: number[] = [];
  for (let t = Math.ceil(t0 / stepMs) * stepMs; t <= t1; t += stepMs) out.push(t);
  return out;
}

/** Local wall-clock time `HH:MM:SS`, or `HH:MM` when `seconds` is false. */
export function fmtClock(ms: number, seconds = true): string {
  return new Date(ms).toLocaleTimeString('sv-SE', {
    hour: '2-digit',
    minute: '2-digit',
    second: seconds ? '2-digit' : undefined,
  });
}
