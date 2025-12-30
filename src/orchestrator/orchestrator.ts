/**
 * Orchestrator実装
 */

import { EventEmitter } from 'events';
import type {
  Task,
  TaskResult,
  ExecutionPattern,
  A2AMessage,
  A2AResponse,
} from '../types/index.js';
import { Subagent } from '../core/subagent.js';
import { TaskParser } from './task-parser.js';
import { Planner } from './planner.js';
import { ResultAggregator, AggregatedResult } from './result-aggregator.js';
import { A2AProtocol } from '../protocol/a2a.js';
import { Logger } from '../utils/logger.js';

export class Orchestrator extends EventEmitter {
  private subagents: Map<string, Subagent>;
  private taskParser: TaskParser;
  private planner: Planner;
  private resultAggregator: ResultAggregator;
  private logger: Logger;
  private activeTasks: Map<string, Task>;

  constructor() {
    super();
    this.subagents = new Map();
    this.taskParser = new TaskParser();
    this.planner = new Planner();
    this.resultAggregator = new ResultAggregator();
    this.logger = new Logger('orchestrator');
    this.activeTasks = new Map();
  }

  /**
   * Subagentを登録
   */
  registerSubagent(subagent: Subagent): void {
    this.subagents.set(subagent.getConfig().id, subagent);
    this.logger.info(`Subagent registered: ${subagent.getConfig().name}`, {
      id: subagent.getConfig().id,
    });

    // イベントリスナーを設定
    subagent.on('progress', (report) => {
      this.emit('subagent_progress', {
        agentId: subagent.getConfig().id,
        ...report,
      });
    });

    subagent.on('status_changed', (data) => {
      this.emit('subagent_status_changed', data);
    });
  }

  /**
   * Subagentを取得
   */
  getSubagent(id: string): Subagent | undefined {
    return this.subagents.get(id);
  }

  /**
   * タスクを実行
   */
  async execute(
    input: string | Task | Task[],
    pattern?: ExecutionPattern
  ): Promise<AggregatedResult> {
    try {
      // 1. タスクを解析
      const tasks = this.taskParser.parse(input);
      this.logger.info(`Parsed ${tasks.length} task(s)`);

      // 2. 実行計画を作成
      const plan = this.planner.createPlan(tasks, pattern);
      this.logger.info(`Created plan with pattern: ${plan.pattern}`);

      // 3. 計画に従って実行
      const results = await this.executePlan(plan);

      // 4. 結果を集約
      const aggregated = this.resultAggregator.aggregate(results, plan.pattern);
      this.logger.info('Execution completed', {
        success: aggregated.success,
        metrics: aggregated.metrics,
      });

      return aggregated;
    } catch (error) {
      this.logger.error('Execution failed', { error });
      throw error;
    }
  }

  /**
   * 実行計画を実行
   */
  private async executePlan(plan: {
    pattern: ExecutionPattern;
    tasks: Task[];
    dependencies?: Map<string, string[]>;
    conditions?: any[];
  }): Promise<TaskResult[]> {
    switch (plan.pattern) {
      case 'sequential':
        return this.executeSequential(plan.tasks, plan.dependencies);
      case 'parallel':
        return this.executeParallel(plan.tasks);
      case 'hierarchical':
        return this.executeHierarchical(plan.tasks, plan.dependencies);
      case 'conditional':
        return this.executeConditional(plan.tasks, plan.conditions);
      default:
        return this.executeParallel(plan.tasks);
    }
  }

  /**
   * Sequential実行
   */
  private async executeSequential(
    tasks: Task[],
    dependencies?: Map<string, string[]>
  ): Promise<TaskResult[]> {
    const results: TaskResult[] = [];
    const resultMap = new Map<string, TaskResult>();

    for (const task of tasks) {
      // 依存関係の結果をコンテキストに追加
      if (dependencies?.has(task.id)) {
        const deps = dependencies.get(task.id)!;
        const depResults = deps.map((depId) => resultMap.get(depId)).filter(Boolean);
        if (!task.context) {
          task.context = {};
        }
        task.context.previous_results = depResults;
      }

      const result = await this.executeTask(task);
      results.push(result);
      resultMap.set(task.id, result);
    }

    return results;
  }

  /**
   * Parallel実行
   */
  private async executeParallel(tasks: Task[]): Promise<TaskResult[]> {
    const promises = tasks.map((task) => this.executeTask(task));
    return Promise.all(promises);
  }

  /**
   * Hierarchical実行
   */
  private async executeHierarchical(
    tasks: Task[],
    dependencies?: Map<string, string[]>
  ): Promise<TaskResult[]> {
    // 親タスクを特定
    const parentTask = tasks.find((task) => !dependencies?.has(task.id));
    const childTasks = tasks.filter((task) => task.id !== parentTask?.id);

    const results: TaskResult[] = [];

    if (parentTask) {
      // 親タスクを実行
      const parentResult = await this.executeTask(parentTask);
      results.push(parentResult);

      // 子タスクを並列実行
      if (childTasks.length > 0) {
        const childResults = await Promise.all(
          childTasks.map((task) => {
            // 親の結果をコンテキストに追加
            if (!task.context) {
              task.context = {};
            }
            task.context.parent_result = parentResult.result;
            return this.executeTask(task);
          })
        );
        results.push(...childResults);
      }
    } else {
      // 親タスクがない場合は並列実行
      return this.executeParallel(tasks);
    }

    return results;
  }

  /**
   * Conditional実行
   */
  private async executeConditional(
    tasks: Task[],
    _conditions?: any[]
  ): Promise<TaskResult[]> {
    const results: TaskResult[] = [];

    for (const task of tasks) {
      // 条件を評価
      if (task.parameters.condition) {
        const shouldExecute = this.evaluateCondition(
          task.parameters.condition,
          results
        );
        if (!shouldExecute) {
          continue;
        }
      }

      const result = await this.executeTask(task);
      results.push(result);
    }

    return results;
  }

  /**
   * 条件を評価
   */
  private evaluateCondition(condition: string, previousResults: TaskResult[]): boolean {
    // 簡易的な条件評価（実際の実装ではより高度な評価を実装）
    try {
      const previous = {
        success: previousResults.every((r) => r.status === 'completed'),
        results: previousResults,
      };

      // シンプルな条件評価（evalの代わりに安全な評価を実装）
      if (condition.includes('previous.success')) {
        return previous.success;
      }
      if (condition.includes('previous.results.length')) {
        const match = condition.match(/previous\.results\.length\s*([><=]+)\s*(\d+)/);
        if (match) {
          const operator = match[1];
          const value = parseInt(match[2], 10);
          switch (operator) {
            case '>':
              return previous.results.length > value;
            case '<':
              return previous.results.length < value;
            case '>=':
              return previous.results.length >= value;
            case '<=':
              return previous.results.length <= value;
            case '===':
            case '==':
              return previous.results.length === value;
            default:
              return true;
          }
        }
      }

      // デフォルトは実行
      return true;
    } catch {
      return true; // 評価に失敗した場合は実行
    }
  }

  /**
   * 単一タスクを実行
   */
  private async executeTask(task: Task): Promise<TaskResult> {
    this.activeTasks.set(task.id, task);
    this.logger.info(`Executing task: ${task.id}`, { action: task.action });

    try {
      // 適切なSubagentを選択
      const subagent = this.selectSubagent(task);
      if (!subagent) {
        throw new Error(`No suitable subagent found for action: ${task.action}`);
      }

      // タスクを実行
      const result = await subagent.execute(task);

      this.activeTasks.delete(task.id);
      return result;
    } catch (error) {
      this.activeTasks.delete(task.id);
      throw error;
    }
  }

  /**
   * 適切なSubagentを選択
   */
  private selectSubagent(task: Task): Subagent | undefined {
    // アクション名に基づいてSubagentを選択
    const actionToAgentMap: Record<string, string> = {
      research: 'research-001',
      codegen: 'codegen-001',
      review: 'review-001',
      document: 'document-001',
    };

    const agentId = actionToAgentMap[task.action] || task.action;
    return this.subagents.get(agentId);
  }

  /**
   * A2Aメッセージを処理
   */
  async handleA2AMessage(message: A2AMessage): Promise<A2AResponse | void> {
    const validation = A2AProtocol.validateMessage(message);
    if (!validation.valid) {
      this.logger.error('Invalid A2A message', { error: validation.error });
      return A2AProtocol.createErrorMessage(
        'orchestrator',
        message.from,
        message.message_id,
        {
          code: 'INVALID_MESSAGE',
          message: validation.error || 'Invalid message format',
        }
      );
    }

    if (message.type === 'task_request') {
      const payload = message.payload as any;
      const task: Task = {
        id: payload.task_id,
        action: payload.action,
        parameters: payload.parameters || {},
        context: payload.context,
        metadata: {
          correlation_id: message.metadata?.correlation_id,
          priority: message.metadata?.priority,
        },
      };

      try {
        const result = await this.executeTask(task);
        return A2AProtocol.createTaskResult(
          'orchestrator',
          message.from,
          message.message_id,
          {
            task_id: task.id,
            result: result.result,
            error: result.error,
          },
          result.status,
          result.metrics
        );
      } catch (error) {
        return A2AProtocol.createErrorMessage(
          'orchestrator',
          message.from,
          message.message_id,
          {
            code: 'EXECUTION_ERROR',
            message: error instanceof Error ? error.message : String(error),
          }
        );
      }
    }

    if (message.type === 'heartbeat') {
      // heartbeatメッセージを受信したが、レスポンスは不要
      A2AProtocol.createHeartbeat('orchestrator', message.from);
      return;
    }
  }
}

