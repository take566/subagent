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
import { ToolRegistry } from './tool.js';
import { Logger } from '../utils/logger.js';
import type { ErrorHandler } from '../utils/error-handler.js';

export abstract class Subagent extends EventEmitter {
  protected id: string;
  protected name: string;
  protected version: string;
  protected tools: ToolRegistry;
  protected state: SubagentState;
  protected config: SubagentConfig;
  protected logger: Logger;
  private errorHandler?: ErrorHandler;
  private escalationConditions: string[] = [];

  constructor(config: SubagentConfig, tools: Tool[] = []) {
    super();
    this.id = config.id;
    this.name = config.name;
    this.version = config.version;
    this.config = config;
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
   * ツールを呼び出し
   */
  protected async callTool(name: string, params: Record<string, any>): Promise<any> {
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
      this.logger.debug(`Tool ${name} completed`, { result });
      return result;
    } catch (error) {
      this.logger.error(`Tool ${name} failed`, { error });
      const alternative = await this.findAlternativeTool(name, error);
      if (!alternative) {
        throw error;
      }
      // 代替ツールは 1 回だけ試す。失敗したら元のエラーを投げる
      try {
        const result = await alternative.invoke(params);
        this.logger.debug(`Alternative tool ${alternative.name} completed`, { result });
        return result;
      } catch (altError) {
        this.logger.error(`Alternative tool ${alternative.name} failed`, { error: altError });
        throw error;
      }
    }
  }

  /**
   * ErrorHandler を設定（未設定なら従来どおりの挙動）
   */
  setErrorHandler(handler: ErrorHandler, escalationConditions: string[] = []): void {
    this.errorHandler = handler;
    this.escalationConditions = escalationConditions;
  }

  /**
   * 結果にエスカレーション要求フラグを付ける（execute の戻り値に通す）
   */
  protected applyEscalation(result: TaskResult): TaskResult {
    if (
      this.errorHandler &&
      this.escalationConditions.length > 0 &&
      this.errorHandler.shouldEscalate(result, this.escalationConditions)
    ) {
      return { ...result, escalate: true };
    }
    return result;
  }

  /**
   * 代替ツールを探す（許可済み・登録済みのものだけ）
   */
  private async findAlternativeTool(name: string, error: unknown): Promise<Tool | undefined> {
    if (!this.errorHandler) return undefined;
    const err = error instanceof Error ? error : new Error(String(error));
    const decision = await this.errorHandler.handleToolFailure(name, err);
    const altName = decision.alternativeTool;
    if (!decision.useAlternative || !altName || altName === name) return undefined;
    const alt = this.tools.get(altName);
    if (!alt || !this.config.tools.includes(altName)) return undefined;
    return alt;
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
   * agent id を上書きする（MCP の AGENT_ID 環境変数用）
   */
  setId(id: string): void {
    this.id = id;
    this.config = { ...this.config, id };
    this.state.id = id;
    this.logger = new Logger(id);
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

