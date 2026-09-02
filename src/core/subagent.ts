/**
 * Subagent基本クラス
 */

import { EventEmitter } from 'events';
import type {
  Task,
  TaskResult,
  SubagentState,
  SubagentConfig,
  Tool,
  ProgressReport,
  AgentStatus,
} from '../types/index.js';
import {
  validateSubagentConfig,
  validateTaskAgainstInterface,
  ValidationError,
} from '../types/validation.js';
import { ToolRegistry } from './tool.js';
import { Logger } from '../utils/logger.js';

export class TimeoutError extends Error {
  readonly code = 'TIMEOUT';

  constructor(timeoutMs: number, taskId?: string) {
    super(
      `Task${taskId ? ` ${taskId}` : ''} timed out after ${timeoutMs}ms`
    );
    this.name = 'TimeoutError';
  }
}

export abstract class Subagent extends EventEmitter {
  protected id: string;
  protected name: string;
  protected version: string;
  protected tools: ToolRegistry;
  protected state: SubagentState;
  protected config: SubagentConfig;
  protected logger: Logger;
  /** 現在実行中タスクの AbortSignal（H-1） */
  protected abortSignal: AbortSignal | null = null;

  constructor(config: SubagentConfig, tools: Tool[] = []) {
    super();
    const validated = validateSubagentConfig(config);
    this.id = validated.id;
    this.name = validated.name;
    this.version = validated.version;
    this.config = validated;
    this.tools = new ToolRegistry();
    this.logger = new Logger(this.id);

    // ツールを登録
    tools.forEach((tool) => this.tools.register(tool));

    // 状態を初期化
    this.state = {
      id: this.id,
      status: 'idle',
      currentTask: null,
      context: {
        memory: new Map(),
        history: [],
      },
      metrics: {
        tasksCompleted: 0,
        avgDuration: 0,
        successRate: 1.0,
      },
    };
  }

  /**
   * タスクを実行（抽象メソッド）
   */
  abstract execute(task: Task): Promise<TaskResult>;

  /**
   * バリデーション + タイムアウト付きでタスクを実行する入口 (F-1 / H-1)
   */
  async run(task: Task): Promise<TaskResult> {
    this.logger.setContext({
      task_id: task.id,
      correlation_id: task.metadata?.correlation_id,
    });

    try {
      validateTaskAgainstInterface(task, this.config);
    } catch (error) {
      if (error instanceof ValidationError) {
        this.logger.error('Task validation failed', {
          task_id: task.id,
          status: 'failed',
          details: error.details,
        });
        return {
          taskId: typeof task?.id === 'string' ? task.id : 'unknown',
          status: 'failed',
          error: {
            code: error.code,
            message: error.message,
            details: error.details,
          },
        };
      }
      throw error;
    }

    const timeoutMs = this.config.behavior.timeout;
    if (!timeoutMs) {
      try {
        return await this.execute(task);
      } finally {
        this.logger.clearContext();
      }
    }

    const controller = new AbortController();
    this.abortSignal = controller.signal;
    const timer = setTimeout(() => controller.abort(), timeoutMs);

    try {
      const result = await Promise.race([
        this.execute(task),
        new Promise<never>((_, reject) => {
          controller.signal.addEventListener(
            'abort',
            () => reject(new TimeoutError(timeoutMs, task.id)),
            { once: true }
          );
        }),
      ]);
      return result;
    } catch (error) {
      if (error instanceof TimeoutError) {
        this.logger.error(error.message, {
          task_id: task.id,
          status: 'failed',
          duration_ms: timeoutMs,
        });
        this.updateStatus('failed');
        return {
          taskId: task.id,
          status: 'failed',
          error: {
            code: error.code,
            message: error.message,
            details: { timeout_ms: timeoutMs },
          },
        };
      }
      throw error;
    } finally {
      clearTimeout(timer);
      this.abortSignal = null;
      this.logger.clearContext();
    }
  }

  /**
   * 現在の AbortSignal を取得（ツール呼び出しへ伝播用）
   */
  protected getAbortSignal(): AbortSignal | null {
    return this.abortSignal;
  }

  /**
   * ツールを呼び出し
   */
  protected async callTool(name: string, params: Record<string, any>): Promise<any> {
    if (this.abortSignal?.aborted) {
      throw new TimeoutError(this.config.behavior.timeout || 0, this.state.currentTask?.id);
    }

    const tool = this.tools.get(name);
    if (!tool) {
      throw new Error(`Tool ${name} not available for ${this.name}`);
    }

    // ツールが利用可能か確認
    if (!this.config.tools.includes(name)) {
      throw new Error(`Tool ${name} is not allowed for ${this.name}`);
    }

    this.logger.debug(`Calling tool: ${name}`, { params });
    try {
      const result = await tool.invoke(params);
      if (this.abortSignal?.aborted) {
        throw new TimeoutError(
          this.config.behavior.timeout || 0,
          this.state.currentTask?.id
        );
      }
      this.logger.debug(`Tool ${name} completed`, { result });
      return result;
    } catch (error) {
      this.logger.error(`Tool ${name} failed`, { error });
      throw error;
    }
  }

  /**
   * 進捗を報告
   */
  protected async reportProgress(progress: number, message: string): Promise<void> {
    const taskId = this.state.currentTask?.id;
    if (!taskId) return;

    const report: ProgressReport = {
      taskId,
      progress: Math.max(0, Math.min(1, progress)),
      message,
      timestamp: new Date().toISOString(),
    };

    this.logger.info(`Progress: ${Math.round(progress * 100)}%`, { message });
    this.emit('progress', report);
  }

  /**
   * 状態を更新
   */
  protected updateStatus(status: AgentStatus): void {
    this.state.status = status;
    this.emit('status_changed', { agentId: this.id, status });
  }

  /**
   * メモリに保存
   */
  protected setMemory(key: string, value: any): void {
    this.state.context.memory.set(key, value);
  }

  /**
   * メモリから取得
   */
  protected getMemory(key: string): any {
    return this.state.context.memory.get(key);
  }

  /**
   * 履歴に追加
   */
  protected addToHistory(result: TaskResult): void {
    this.state.context.history.push(result);
    this.updateMetrics(result);
  }

  /**
   * メトリクスを更新
   */
  private updateMetrics(result: TaskResult): void {
    const metrics = this.state.metrics;
    metrics.tasksCompleted += 1;

    if (result.metrics?.duration_ms) {
      const totalDuration = metrics.avgDuration * (metrics.tasksCompleted - 1) + result.metrics.duration_ms;
      metrics.avgDuration = totalDuration / metrics.tasksCompleted;
    }

    const successCount = this.state.context.history.filter((r) => r.status === 'completed').length;
    metrics.successRate = successCount / metrics.tasksCompleted;
  }

  /**
   * 現在の状態を取得
   */
  getState(): SubagentState {
    return { ...this.state };
  }

  /**
   * 設定を取得
   */
  getConfig(): SubagentConfig {
    return { ...this.config };
  }

  /**
   * 利用可能なツール名を取得
   */
  getAvailableTools(): string[] {
    return this.tools.getNames();
  }

  /**
   * ツールを取得（MCP統合用）
   */
  getTool(name: string): Tool | undefined {
    return this.tools.get(name);
  }

  /**
   * ツールを呼び出し（MCP統合用）
   */
  async invokeTool(name: string, params: Record<string, any>): Promise<any> {
    return this.callTool(name, params);
  }
}

