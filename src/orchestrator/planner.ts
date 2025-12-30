/**
 * Planner実装
 */

import type {
  Task,
  ExecutionPattern,
  OrchestrationPlan,
  ConditionalRule,
} from '../types/index.js';
import { Logger } from '../utils/logger.js';

export class Planner {
  private logger: Logger;

  constructor() {
    this.logger = new Logger('planner');
  }

  /**
   * 実行計画を作成
   */
  createPlan(tasks: Task[], pattern?: ExecutionPattern): OrchestrationPlan {
    // パターンが指定されていない場合は自動判定
    const executionPattern = pattern || this.detectPattern(tasks);

    switch (executionPattern) {
      case 'sequential':
        return this.createSequentialPlan(tasks);
      case 'parallel':
        return this.createParallelPlan(tasks);
      case 'hierarchical':
        return this.createHierarchicalPlan(tasks);
      case 'conditional':
        return this.createConditionalPlan(tasks);
      default:
        return this.createParallelPlan(tasks);
    }
  }

  /**
   * 実行パターンを自動検出
   */
  private detectPattern(tasks: Task[]): ExecutionPattern {
    // 依存関係がある場合はsequential
    const hasDependencies = tasks.some(
      (task) => task.context?.parent_task || task.metadata?.correlation_id
    );

    if (hasDependencies) {
      return 'sequential';
    }

    // 条件分岐がある場合はconditional
    const hasConditions = tasks.some((task) => task.parameters.condition);

    if (hasConditions) {
      return 'conditional';
    }

    // デフォルトはparallel
    return 'parallel';
  }

  /**
   * Sequential計画を作成
   */
  private createSequentialPlan(tasks: Task[]): OrchestrationPlan {
    // 依存関係に基づいて順序を決定
    const orderedTasks = this.topologicalSort(tasks);
    const dependencies = new Map<string, string[]>();

    for (let i = 1; i < orderedTasks.length; i++) {
      const prevTask = orderedTasks[i - 1];
      dependencies.set(orderedTasks[i].id, [prevTask.id]);
    }

    return {
      pattern: 'sequential',
      tasks: orderedTasks,
      dependencies,
    };
  }

  /**
   * Parallel計画を作成
   */
  private createParallelPlan(tasks: Task[]): OrchestrationPlan {
    return {
      pattern: 'parallel',
      tasks,
    };
  }

  /**
   * Hierarchical計画を作成
   */
  private createHierarchicalPlan(tasks: Task[]): OrchestrationPlan {
    // 親タスクを特定
    const parentTask = tasks.find((task) => !task.context?.parent_task);
    const childTasks = tasks.filter((task) => task.context?.parent_task === parentTask?.id);

    const dependencies = new Map<string, string[]>();
    if (parentTask) {
      childTasks.forEach((child) => {
        dependencies.set(child.id, [parentTask.id]);
      });
    }

    return {
      pattern: 'hierarchical',
      tasks,
      dependencies,
    };
  }

  /**
   * Conditional計画を作成
   */
  private createConditionalPlan(tasks: Task[]): OrchestrationPlan {
    const conditions: ConditionalRule[] = [];

    tasks.forEach((task) => {
      if (task.parameters.condition) {
        conditions.push({
          condition: task.parameters.condition,
          target_subagent: task.parameters.target_subagent || 'default',
          action: task.action,
        });
      }
    });

    return {
      pattern: 'conditional',
      tasks,
      conditions,
    };
  }

  /**
   * トポロジカルソート（依存関係に基づく順序付け）
   */
  private topologicalSort(tasks: Task[]): Task[] {
    const sorted: Task[] = [];
    const visited = new Set<string>();
    const visiting = new Set<string>();

    const visit = (task: Task) => {
      if (visiting.has(task.id)) {
        this.logger.warn(`Circular dependency detected for task ${task.id}`);
        return;
      }
      if (visited.has(task.id)) {
        return;
      }

      visiting.add(task.id);

      // 依存タスクを先に処理
      const dependencies = this.getDependencies(task, tasks);
      dependencies.forEach((dep) => {
        const depTask = tasks.find((t) => t.id === dep);
        if (depTask) {
          visit(depTask);
        }
      });

      visiting.delete(task.id);
      visited.add(task.id);
      sorted.push(task);
    };

    tasks.forEach((task) => {
      if (!visited.has(task.id)) {
        visit(task);
      }
    });

    return sorted;
  }

  /**
   * タスクの依存関係を取得
   */
  private getDependencies(task: Task, allTasks: Task[]): string[] {
    const deps: string[] = [];

    if (task.context?.parent_task) {
      deps.push(task.context.parent_task);
    }

    // correlation_idに基づく依存関係
    if (task.metadata?.correlation_id) {
      const relatedTasks = allTasks.filter(
        (t) =>
          t.metadata?.correlation_id === task.metadata?.correlation_id &&
          t.id !== task.id &&
          !deps.includes(t.id)
      );
      deps.push(...relatedTasks.map((t) => t.id));
    }

    return deps;
  }
}

