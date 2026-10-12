import { createLogger } from './index.mjs';

const logger = createLogger({ port: Number(process.env.RUAL_PORT || 7331), source: 'node-example' });
logger.info('Application started', { port: 3000 });
logger.error('Request failed', { request_id: 'demo-42', error: { stack: 'Error: demo\n  at example.mjs' } });
await logger.close();
