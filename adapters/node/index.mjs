import net from 'node:net';

const MAX_FRAME = 64 * 1024;
const LEVELS = new Set(['TRACE', 'DEBUG', 'INFO', 'WARN', 'WARNING', 'ERROR', 'FATAL', 'CRITICAL', 'SUCCESS']);

/** Forward logs without making the application wait for a socket write. */
export function createLogger({ host = '127.0.0.1', port = 7331, source = 'node-app' } = {}) {
  if (!['127.0.0.1', '::1'].includes(host)) throw new Error('rual host must be a loopback IP');
  if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error('invalid port');
  if (!source || Buffer.byteLength(source) > 128) throw new Error('invalid source name');
  const queue = [];
  let socket, retry, connected = false, blocked = false, closing = false, closed = false;
  let dropped = 0, reported = 0, resolveClose, closePromise, closeTimeout;

  function finish() {
    if (closed) return;
    closed = true;
    clearTimeout(retry);
    clearTimeout(closeTimeout);
    dropped += queue.length;
    queue.length = 0;
    socket?.destroy();
    resolveClose?.();
  }

  function pump() {
    if (!connected || blocked || closed) return;
    if (dropped > reported) {
      blocked = !socket.write(JSON.stringify({ type: 'dropped', count: dropped - reported }) + '\n');
      reported = dropped;
    }
    while (queue.length && !blocked) {
      blocked = !socket.write(queue.shift());
    }
    if (closing && queue.length === 0 && !blocked) socket.end();
  }

  function connect() {
    if (closed) return;
    socket = net.createConnection({ host, port });
    socket.unref();
    socket.setTimeout(200, () => socket.destroy());
    socket.on('error', () => {}); // Connection errors must not become application errors.
    socket.on('connect', () => {
      connected = true;
      blocked = false;
      socket.setTimeout(0);
      socket.write(JSON.stringify({ type: 'register', version: 1, source, language: 'node' }) + '\n');
      pump();
    });
    socket.on('drain', () => { blocked = false; pump(); });
    socket.on('close', () => {
      connected = false;
      if (closing && queue.length === 0) { finish(); return; }
      if (!closed) { retry = setTimeout(connect, 200); retry.unref(); }
    });
  }
  connect();

  const logger = {
    get dropped() { return dropped; },
    log(level, message, fields = {}) {
      let frame;
      try {
        level = level.toUpperCase();
        if (!LEVELS.has(level) || typeof message !== 'string' || !fields || Array.isArray(fields)
            || typeof fields !== 'object') throw new Error('invalid record');
        frame = JSON.stringify({ type: 'log', timestamp: new Date().toISOString(), level, message, fields }) + '\n';
      } catch { dropped++; return false; }
      if (closing || queue.length >= 64 || Buffer.byteLength(frame) > MAX_FRAME) { dropped++; return false; }
      queue.push(frame);
      pump();
      return true;
    },
    close() {
      if (closePromise) return closePromise;
      closing = true;
      closePromise = new Promise(resolve => { resolveClose = resolve; });
      closeTimeout = setTimeout(finish, 500);
      socket?.ref();
      pump();
      return closePromise;
    },
  };
  for (const level of ['trace', 'debug', 'info', 'warn', 'error', 'success']) {
    logger[level] = (message, fields) => logger.log(level, message, fields);
  }
  return logger;
}
