// Ports for the static server of a gate run: a free one per run, so two worktrees can run the
// gates at the same time.
import { execFileSync } from 'node:child_process';

const PICK =
  "const s = require('node:net').createServer(); s.listen(0, '127.0.0.1', () => { process.stdout.write(String(s.address().port)); s.close(); });";

/** A TCP port on 127.0.0.1 that nothing listens on now. */
export function freePort(): number {
  const port = Number(execFileSync(process.execPath, ['-e', PICK], { encoding: 'utf8' }));
  if (!Number.isInteger(port) || port <= 0) throw new Error(`no free port: ${port}`);
  return port;
}

/**
 * The port of this gate run: `UX_PORT` when set, else a free port written to `UX_PORT`, so
 * Playwright's worker processes, which load the config again, read the same one.
 */
export function gatePort(): number {
  if (!process.env.UX_PORT) process.env.UX_PORT = String(freePort());
  return Number(process.env.UX_PORT);
}
