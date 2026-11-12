import assert from 'node:assert/strict';
import { createServer } from 'node:net';
import { test } from 'node:test';
import { freePort, gatePort } from './port.ts';

test('a free port can be bound', async () => {
  const port = freePort();
  const s = createServer();
  await new Promise<void>((ok, fail) => {
    s.once('error', fail);
    s.listen(port, '127.0.0.1', ok);
  });
  s.close();
});

test('the gate port comes from UX_PORT, so the workers share the runner port', () => {
  const before = process.env.UX_PORT;
  try {
    delete process.env.UX_PORT;
    const first = gatePort();
    assert.equal(process.env.UX_PORT, String(first));
    assert.equal(gatePort(), first);
    process.env.UX_PORT = '45678';
    assert.equal(gatePort(), 45678);
  } finally {
    if (before === undefined) delete process.env.UX_PORT;
    else process.env.UX_PORT = before;
  }
});
