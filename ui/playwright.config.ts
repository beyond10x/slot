import { defineConfig } from '@playwright/test';
import { gatePort } from './ux/port.ts';

const PORT = gatePort();

export default defineConfig({
  testDir: 'ux',
  testMatch: '*.spec.ts',
  fullyParallel: true,
  workers: 2,
  reporter: [['line'], ['./ux/slowest.ts']],
  timeout: 60_000,
  reportSlowTests: { max: 10, threshold: 5_000 },
  use: {
    baseURL: `http://127.0.0.1:${PORT}/`,
    launchOptions: process.env.UX_CHROME ? { executablePath: process.env.UX_CHROME } : {},
  },
  webServer: {
    command: `pnpm exec vite preview --host 127.0.0.1 --port ${PORT} --strictPort`,
    url: `http://127.0.0.1:${PORT}/`,
    reuseExistingServer: false,
  },
});
