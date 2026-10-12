export interface LoggerOptions {
  host?: string;
  token?: string;
  port?: number;
  source?: string;
}
export interface Logger {
  readonly dropped: number;
  log(level: string, message: string, fields?: Record<string, unknown>): boolean;
  trace(message: string, fields?: Record<string, unknown>): boolean;
  debug(message: string, fields?: Record<string, unknown>): boolean;
  info(message: string, fields?: Record<string, unknown>): boolean;
  warn(message: string, fields?: Record<string, unknown>): boolean;
  error(message: string, fields?: Record<string, unknown>): boolean;
  success(message: string, fields?: Record<string, unknown>): boolean;
  close(): Promise<void>;
}
export function createLogger(options?: LoggerOptions): Logger;
