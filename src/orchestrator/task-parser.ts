/**
 * Task Parser実装
 */

import type { Task } from '../types/index.js';

export class TaskParser {
  constructor() {
    // Loggerは将来の拡張用に保持
  }

  /**
   * タスクを解析して構造化
   */
  parse(input: string | Task | Task[]): Task[] {
    if (Array.isArray(input)) {
      return input.map((task) => this.normalizeTask(task));
    }

    if (typeof input === 'string') {
      return this.parseFromString(input);
    }

    return [this.normalizeTask(input)];
  }

  /**
   * 文字列からタスクを解析
   */
  private parseFromString(input: string): Task[] {
    try {
      const parsed = JSON.parse(input);
      if (Array.isArray(parsed)) {
        return parsed.map((task) => this.normalizeTask(task));
      }
      return [this.normalizeTask(parsed)];
    } catch (error) {
      // JSONでない場合は、シンプルなタスクとして扱う
      return [
        {
          id: this.generateTaskId(),
          action: 'execute',
          parameters: { query: input },
        },
      ];
    }
  }

  /**
   * タスクを正規化
   */
  private normalizeTask(task: Task | Partial<Task>): Task {
    return {
      id: task.id || this.generateTaskId(),
      action: task.action || 'execute',
      parameters: task.parameters || {},
      context: task.context,
      metadata: {
        ...task.metadata,
        created_at: task.metadata?.created_at || new Date().toISOString(),
      },
    };
  }

  /**
   * タスクIDを生成
   */
  private generateTaskId(): string {
    return `task-${Date.now()}-${Math.random().toString(36).substr(2, 9)}`;
  }

  /**
   * タスクの依存関係を抽出
   */
  extractDependencies(tasks: Task[]): Map<string, string[]> {
    const dependencies = new Map<string, string[]>();

    for (const task of tasks) {
      const deps: string[] = [];
      if (task.context?.parent_task) {
        deps.push(task.context.parent_task);
      }
      if (task.metadata?.correlation_id) {
        // 同じcorrelation_idを持つ他のタスクを依存関係として扱う
        const relatedTasks = tasks.filter(
          (t) => t.metadata?.correlation_id === task.metadata?.correlation_id && t.id !== task.id
        );
        deps.push(...relatedTasks.map((t) => t.id));
      }
      if (deps.length > 0) {
        dependencies.set(task.id, deps);
      }
    }

    return dependencies;
  }
}

