// Playwright reporter: every run ends with its ten slowest tests, slowest first, so a test near the 60 s bound is seen.
import type { Reporter, TestCase, TestResult } from '@playwright/test/reporter';

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

export default class Slowest implements Reporter {
  private runs: Timed[] = [];

  onTestEnd(test: TestCase, result: TestResult): void {
    this.runs.push({ title: test.title, ms: result.duration });
  }

  onEnd(): void {
    console.log(['Slowest tests:', ...slowest(this.runs)].join('\n'));
  }

  printsToStdio(): boolean {
    return false;
  }
}
