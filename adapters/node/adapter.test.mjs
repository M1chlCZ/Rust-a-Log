import assert from 'node:assert/strict';
import { test } from 'node:test';
import net from 'node:net';
import { once } from 'node:events';
import { createLogger } from './index.mjs';
import transport from './pino.mjs';

test('registration, fields, multiline messages, and bounded close', async () => {
  const server = net.createServer();
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const received = [];
  const connected = once(server, 'connection');
  const logger = createLogger({ port: server.address().port, source: 'web' });
  const [socket] = await connected;
  let pending = '';
  socket.on('data', chunk => {
    pending += chunk;
    const lines = pending.split('\n');
    pending = lines.pop();
    received.push(...lines.map(line => JSON.parse(line)));
  });
  logger.error('one\ntwo', { request: 42 });
  await logger.close();
  await new Promise(resolve => setTimeout(resolve, 25));
  socket.destroy();
  server.close();
  assert.equal(received[0].type, 'register');
  assert.equal(received[1].message, 'one\ntwo');
  assert.equal(received[1].fields.request, 42);
});

test('unavailable receiver and oversize records do not crash or block the app', async () => {
  assert.throws(() => createLogger({ host: '0.0.0.0' }), /loopback/);
  const server = net.createServer();
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const port = server.address().port;
  await new Promise(resolve => server.close(resolve));
  const logger = createLogger({ port });
  for (let i = 0; i < 500; i++) logger.info('offline');
  logger.info('x'.repeat(70000));
  await logger.close();
  assert.ok(logger.dropped > 0);
});

test('Pino transport preserves split UTF-8 and structured fields', async () => {
  const server = net.createServer();
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const received = [];
  const connected = once(server, 'connection');
  const stream = transport({ port: server.address().port, source: 'pino' });
  const [socket] = await connected;
  let pending = '';
  socket.on('data', chunk => {
    pending += chunk;
    const lines = pending.split('\n');
    pending = lines.pop();
    received.push(...lines.map(line => JSON.parse(line)));
  });
  const bytes = Buffer.from('{"level":50,"msg":"ž failure","request":42}\n');
  const position = bytes.indexOf(Buffer.from('ž'));
  stream.write(bytes.subarray(0, position + 1));
  stream.end(bytes.subarray(position + 1));
  await once(stream, 'finish');
  await new Promise(resolve => setTimeout(resolve, 25));
  socket.destroy();
  server.close();
  assert.equal(received[1].level, 'ERROR');
  assert.equal(received[1].message, 'ž failure');
  assert.equal(received[1].fields.request, 42);
});

test('sender reconnects after an established connection closes', async () => {
  const server = net.createServer();
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const connected = once(server, 'connection');
  const logger = createLogger({ port: server.address().port, source: 'reconnect' });
  const [first] = await connected;
  const again = once(server, 'connection');
  first.destroy();
  const [second] = await again;
  let data = '';
  second.on('data', chunk => { data += chunk; });
  logger.info('after reconnect');
  await logger.close();
  await new Promise(resolve => setTimeout(resolve, 25));
  second.destroy();
  server.close();
  assert.ok(data.includes('"type":"register"') && data.includes('after reconnect'));
});
