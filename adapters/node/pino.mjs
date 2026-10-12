import { Writable } from 'node:stream';
import { StringDecoder } from 'node:string_decoder';
import { createLogger } from './index.mjs';

/** A Pino transport. Pino runs this sender in its transport worker. */
export default function transport(options) {
  const logger = createLogger(options);
  let pending = '';
  const decoder = new StringDecoder('utf8');
  const levels = { 10: 'TRACE', 20: 'DEBUG', 30: 'INFO', 40: 'WARNING', 50: 'ERROR', 60: 'ERROR' };
  return new Writable({
    write(chunk, encoding, callback) {
      pending += decoder.write(chunk);
      const lines = pending.split('\n');
      pending = lines.pop();
      for (const line of lines) {
        try {
          const { level, msg, time, ...fields } = JSON.parse(line);
          if (time !== undefined) fields.pino_time = time;
          logger.log(levels[level] || 'INFO', String(msg ?? ''), fields);
        } catch { logger.log('INFO', line); }
      }
      if (Buffer.byteLength(pending) > 64 * 1024) { logger.log('INFO', pending); pending = ''; }
      callback();
    },
    final(callback) {
      pending += decoder.end();
      if (pending) logger.log('INFO', pending);
      logger.close().then(() => callback(), callback);
    },
  });
}
