import { defineConfig } from '@playwright/test';
import { gatePort } from '../ui/ux/port.ts';

const PORT = gatePort();
/** The built site to measure; a round measures the site before its changes from another directory. */
const BUILD = process.env.UX_BUILD ?? 'build';

export default defineConfig({
  testDir: 'ux',
  testMatch: '*.spec.ts',
  fullyParallel: true,
  workers: 2,
  reporter: [['line'], ['../ui/ux/slowest.ts']],
  timeout: 60_000,
  use: {
    baseURL: `http://127.0.0.1:${PORT}/slot/`,
    launchOptions: process.env.UX_CHROME ? { executablePath: process.env.UX_CHROME } : {},
  },
  webServer: {
    command: `node --disable-warning=ExperimentalWarning ../ui/ux/serve.ts ${BUILD} ${PORT} --base /slot/ --docs`,
    url: `http://127.0.0.1:${PORT}/slot/`,
    reuseExistingServer: false,
  },
});
