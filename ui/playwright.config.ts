import { defineConfig } from '@playwright/test';

const PORT = 4317;

export default defineConfig({
  testDir: 'ux',
  testMatch: '*.spec.ts',
  fullyParallel: true,
  workers: 2,
  reporter: [['line']],
  timeout: 120_000,
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
