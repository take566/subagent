/**
 * Result Aggregator実装
 */

import type { TaskResult, ExecutionPattern } from '../types/index.js';

export interface AggregatedResult {
  success: boolean;
  results: TaskResult[];
  aggregated: any;
  metrics: {
    total_duration_ms: number;
    success_count: number;
    failure_count: number;
    avg_confidence: number;
  };
}

export class ResultAggregator {
  constructor() {
    // Loggerは将来の拡張用に保持
  }

  /**
   * 結果を集約
   */
  aggregate(
    results: TaskResult[],
    pattern: ExecutionPattern = 'parallel',
    dependencies?: Map<string, string[]>
  ): AggregatedResult {
    const success = results.every((r) => r.status === 'completed');
    const successCount = results.filter((r) => r.status === 'completed').length;
    const failureCount = results.length - successCount;

    const totalDuration = results.reduce(
      (sum, r) => sum + (r.metrics?.duration_ms || 0),
      0
    );

    const confidences = results
      .map((r) => r.confidence || 0)
      .filter((c) => c > 0);
    const avgConfidence =
      confidences.length > 0
        ? confidences.reduce((sum, c) => sum + c, 0) / confidences.length
        : 0;

    let aggregated: any;

    switch (pattern) {
      case 'sequential':
        aggregated = this.aggregateSequential(results);
        break;
      case 'parallel':
        aggregated = this.aggregateParallel(results);
        break;
      case 'hierarchical':
        aggregated = this.aggregateHierarchical(results, dependencies);
        break;
      case 'conditional':
        aggregated = this.aggregateConditional(results);
        break;
      default:
        aggregated = this.aggregateParallel(results);
    }

    return {
      success,
      results,
      aggregated,
      metrics: {
        total_duration_ms: totalDuration,
        success_count: successCount,
        failure_count: failureCount,
        avg_confidence: avgConfidence,
      },
    };
  }

  /**
   * Sequential結果を集約
   */
  private aggregateSequential(results: TaskResult[]): any {
    // 最後の結果を返す（前の結果は次の入力として使用されたと仮定）
    const lastResult = results[results.length - 1];
    return {
      final_result: lastResult.result,
      chain: results.map((r) => ({
        task_id: r.taskId,
        result: r.result,
      })),
    };
  }

  /**
   * Parallel結果を集約
   */
  private aggregateParallel(results: TaskResult[]): any {
    return {
      results: results.map((r) => ({
        task_id: r.taskId,
        result: r.result,
        status: r.status,
      })),
      combined: this.combineResults(results),
    };
  }

  /**
   * 結果を結合
   */
  private combineResults(results: TaskResult[]): any {
    const combined: Record<string, any> = {};

    results.forEach((result) => {
      if (result.result) {
        Object.assign(combined, result.result);
      }
    });

    return combined;
  }

  /**
   * Hierarchical結果を集約
   */
  private aggregateHierarchical(
    results: TaskResult[],
    dependencies?: Map<string, string[]>
  ): any {
    // dependencies が無ければ従来どおり先頭を親とする（orchestrator は親を先頭に積む）
    const isChild = (r: TaskResult): boolean =>
      dependencies ? dependencies.has(r.taskId) : false;
    const parent = dependencies
      ? results.find((r) => !isChild(r))
      : results[0];

    if (!parent) {
      return { parent: undefined, children: [] };
    }

    const others = results.filter((r) => r !== parent);
    const ids = new Set(results.map((r) => r.taskId));

    // 子を、結果に含まれる最初の依存先の下にネストする。
    // 依存先が不明・不在・循環のものは親直下に置く。
    const nodeOf = (r: TaskResult): any => ({
      task_id: r.taskId,
      result: r.result,
      children: [] as any[],
    });
    const nodes = new Map<string, any>();
    others.forEach((r) => nodes.set(r.taskId, nodeOf(r)));

    const parentOf = (r: TaskResult): string => {
      const dep = dependencies
        ?.get(r.taskId)
        ?.find((d) => ids.has(d) && d !== r.taskId);
      return dep ?? parent.taskId;
    };

    // 循環検出: 親へ辿り着けないものは親直下に付け替える
    const reachesParent = (id: string): boolean => {
      const seen = new Set<string>();
      let cur: string | undefined = id;
      while (cur && cur !== parent.taskId) {
        if (seen.has(cur)) return false;
        seen.add(cur);
        const r = others.find((o) => o.taskId === cur);
        cur = r ? parentOf(r) : undefined;
      }
      return cur === parent.taskId;
    };

    const children: any[] = [];
    others.forEach((r) => {
      const node = nodes.get(r.taskId);
      const pid = reachesParent(r.taskId) ? parentOf(r) : parent.taskId;
      const holder = pid === parent.taskId ? undefined : nodes.get(pid);
      (holder ? holder.children : children).push(node);
    });

    return { parent: parent.result, parent_task_id: parent.taskId, children };
  }

  /**
   * Conditional結果を集約
   */
  private aggregateConditional(results: TaskResult[]): any {
    return {
      executed: results.map((r) => ({
        task_id: r.taskId,
        result: r.result,
        status: r.status,
      })),
    };
  }
}

