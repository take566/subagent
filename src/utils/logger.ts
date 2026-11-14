/**
 * ロガー実装
 */

export enum LogLevel {
  DEBUG = 0,
  INFO = 1,
  WARN = 2,
  ERROR = 3,
}

export interface LogEntry {
  timestamp: string;
  level: string;
  agent_id?: string;
  task_id?: string;
  correlation_id?: string;
  duration?: number;
  status?: string;
  message: string;
  data?: any;
}

/** child() で束ねるログコンテキスト (設計 §11) */
export interface LogContext {
  task_id?: string;
  correlation_id?: string;
}

/** 各ログ呼び出しで付与できる §11 フィールド */
export interface LogFields {
  duration?: number; // ms
  status?: string;
}

export class Logger {
  private agentId?: string;
  private level: LogLevel;
  private context: LogContext;

  constructor(agentId?: string, level: LogLevel = LogLevel.INFO, context: LogContext = {}) {
    this.agentId = agentId;
    this.level = level;
    this.context = context;
  }

  /** task_id / correlation_id を固定した子ロガーを返す */
  child(context: LogContext): Logger {
    return new Logger(this.agentId, this.level, { ...this.context, ...context });
  }

  private log(level: LogLevel, message: string, data?: any, fields: LogFields = {}): void {
    if (level < this.level) return;

    const entry: LogEntry = {
      timestamp: new Date().toISOString(),
      level: LogLevel[level],
      agent_id: this.agentId,
      task_id: this.context.task_id,
      correlation_id: this.context.correlation_id,
      duration: fields.duration,
      status: fields.status,
      message,
      data,
    };

    const logMethod = this.getLogMethod(level);
    logMethod(JSON.stringify(entry, null, 2));
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

  debug(message: string, data?: any, fields?: LogFields): void {
    this.log(LogLevel.DEBUG, message, data, fields);
  }

  info(message: string, data?: any, fields?: LogFields): void {
    this.log(LogLevel.INFO, message, data, fields);
  }

  warn(message: string, data?: any, fields?: LogFields): void {
    this.log(LogLevel.WARN, message, data, fields);
  }

  error(message: string, data?: any, fields?: LogFields): void {
    this.log(LogLevel.ERROR, message, data, fields);
  }

  setLevel(level: LogLevel): void {
    this.level = level;
  }
}

