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
  message: string;
  data?: any;
}

export class Logger {
  private agentId?: string;
  private level: LogLevel;

  constructor(agentId?: string, level: LogLevel = LogLevel.INFO) {
    this.agentId = agentId;
    this.level = level;
  }

  private log(level: LogLevel, message: string, data?: any): void {
    if (level < this.level) return;

    const entry: LogEntry = {
      timestamp: new Date().toISOString(),
      level: LogLevel[level],
      agent_id: this.agentId,
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

  debug(message: string, data?: any): void {
    this.log(LogLevel.DEBUG, message, data);
  }

  info(message: string, data?: any): void {
    this.log(LogLevel.INFO, message, data);
  }

  warn(message: string, data?: any): void {
    this.log(LogLevel.WARN, message, data);
  }

  error(message: string, data?: any): void {
    this.log(LogLevel.ERROR, message, data);
  }

  setLevel(level: LogLevel): void {
    this.level = level;
  }
}

