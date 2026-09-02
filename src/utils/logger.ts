/**
 * 構造化ログ (L-1)
 */

export enum LogLevel {
  DEBUG = 0,
  INFO = 1,
  WARN = 2,
  ERROR = 3,
}

export interface LogContext {
  task_id?: string;
  correlation_id?: string;
  status?: string;
  duration_ms?: number;
  [key: string]: unknown;
}

export interface LogEntry {
  timestamp: string;
  level: string;
  agent_id?: string;
  task_id?: string;
  correlation_id?: string;
  status?: string;
  duration_ms?: number;
  message: string;
  data?: unknown;
}

export class Logger {
  private agentId?: string;
  private level: LogLevel;
  private context: LogContext = {};

  constructor(agentId?: string, level: LogLevel = LogLevel.INFO) {
    this.agentId = agentId;
    this.level = level;
  }

  /**
   * リクエストスコープのコンテキストを設定（task_id / correlation_id 等）
   */
  setContext(ctx: LogContext): void {
    this.context = { ...this.context, ...ctx };
  }

  clearContext(): void {
    this.context = {};
  }

  getContext(): LogContext {
    return { ...this.context };
  }

  private log(level: LogLevel, message: string, data?: unknown): void {
    if (level < this.level) return;

    const dataObj =
      data && typeof data === 'object' && !Array.isArray(data)
        ? (data as Record<string, unknown>)
        : undefined;

    const entry: LogEntry = {
      timestamp: new Date().toISOString(),
      level: LogLevel[level],
      agent_id: this.agentId,
      task_id:
        (dataObj?.task_id as string | undefined) ?? this.context.task_id,
      correlation_id:
        (dataObj?.correlation_id as string | undefined) ??
        this.context.correlation_id,
      status: (dataObj?.status as string | undefined) ?? this.context.status,
      duration_ms:
        (dataObj?.duration_ms as number | undefined) ?? this.context.duration_ms,
      message,
      data,
    };

    // 空フィールドを落とす
    if (entry.task_id === undefined) delete entry.task_id;
    if (entry.correlation_id === undefined) delete entry.correlation_id;
    if (entry.status === undefined) delete entry.status;
    if (entry.duration_ms === undefined) delete entry.duration_ms;
    if (entry.agent_id === undefined) delete entry.agent_id;
    if (entry.data === undefined) delete entry.data;

    const logMethod = this.getLogMethod(level);
    logMethod(JSON.stringify(entry));
  }

  private getLogMethod(level: LogLevel): (message: string) => void {
    switch (level) {
      case LogLevel.DEBUG:
        return console.debug;
      case LogLevel.INFO:
        return console.info;
      case LogLevel.WARN:
        return console.warn;
      case LogLevel.ERROR:
        return console.error;
      default:
        return console.log;
    }
  }

  debug(message: string, data?: unknown): void {
    this.log(LogLevel.DEBUG, message, data);
  }

  info(message: string, data?: unknown): void {
    this.log(LogLevel.INFO, message, data);
  }

  warn(message: string, data?: unknown): void {
    this.log(LogLevel.WARN, message, data);
  }

  error(message: string, data?: unknown): void {
    this.log(LogLevel.ERROR, message, data);
  }

  setLevel(level: LogLevel): void {
    this.level = level;
  }
}
