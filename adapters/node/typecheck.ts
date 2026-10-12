import { createLogger, type Logger } from "@rust-a-log/node";

const logger: Logger = createLogger({ source: "typecheck", host: "127.0.0.1" });
logger.error("Request failed", { request_id: 42 });
await logger.close();
