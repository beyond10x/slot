// Playwright reporter: every run ends with its ten slowest tests, slowest first, so a test near the 60 s bound is seen.
// A run about to be stopped prints them early, with the tests it is in: 1 s before the step deadline
// (STEP_DEADLINE_AT, set by ux/deadline.ts) and at SIGINT. Playwright's own end of a stopped run can
// come after the launchers above it left, and then its output is lost.
import type { Reporter, TestCase, TestResult } from '@playwright/test/reporter';

/** The report is printed this long before the deadline of the step. */
export const BEFORE_DEADLINE_MS = 1_000;

export interface Timed {
  title: string;
  ms: number;
}

/** One line per test, slowest first, at most `max`. */
export const slowest = (runs: Timed[], max = 10): string[] =>
  [...runs]
    .sort((a, b) => b.ms - a.ms)
    .slice(0, max)
    .map((r) => `  ${(r.ms / 1000).toFixed(1)} s  ${r.title}`);

/** What a run prints at its end: the slowest finished tests, and the tests still running when a run is about to be stopped. */
export const report = (done: Timed[], running: Timed[] = []): string[] => [
  'Slowest tests:',
  ...slowest(done),
  ...(running.length ? ['Still running:', ...slowest(running, running.length)] : []),
];

export default class Slowest implements Reporter {
  private runs: Timed[] = [];
  private started = new Map<TestCase, number>();
  private printed = false;
  private timer: NodeJS.Timeout | undefined;
  private onSignal = (): void => this.print(true);

  onBegin(): void {
    process.once('SIGINT', this.onSignal);
    const at = Number(process.env.STEP_DEADLINE_AT);
    if (at > 0) {
      this.timer = setTimeout(this.onSignal, Math.max(0, at - Date.now() - BEFORE_DEADLINE_MS));
      this.timer.unref();
    }
  }

  onTestBegin(test: TestCase): void {
    this.started.set(test, Date.now());
  }

  onTestEnd(test: TestCase, result: TestResult): void {
    this.started.delete(test);
    this.runs.push({ title: test.title, ms: result.duration });
  }

  onEnd(): void {
    process.off('SIGINT', this.onSignal);
    clearTimeout(this.timer);
    // Also after an early report: the run ended by itself, and this one has every test.
    this.printed = false;
    this.print(false);
  }

  private print(stopped: boolean): void {
    if (this.printed) return;
    this.printed = true;
    const now = Date.now();
    const running = stopped ? [...this.started].map(([t, at]) => ({ title: t.title, ms: now - at })) : [];
    console.log(report(this.runs, running).join('\n'));
  }

  printsToStdio(): boolean {
    return false;
  }
}
