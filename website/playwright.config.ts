import { defineConfig } from '@playwright/test';

const PORT = 4318;
/** The built site to measure; a round measures the site before its changes from another directory. */
const BUILD = process.env.UX_BUILD ?? 'build';

export default defineConfig({
  testDir: 'ux',
  testMatch: '*.spec.ts',
  fullyParallel: true,
  workers: 2,
  reporter: [['line']],
  timeout: 120_000,
  use: {
    baseURL: `http://127.0.0.1:${PORT}/slot/`,
    launchOptions: process.env.UX_CHROME ? { executablePath: process.env.UX_CHROME } : {},
  },
  webServer: {
    command: `npx docusaurus serve --dir ${BUILD} --host 127.0.0.1 --port ${PORT} --no-open`,
    url: `http://127.0.0.1:${PORT}/slot/`,
    reuseExistingServer: false,
  },
});
