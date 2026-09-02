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
    pattern: ExecutionPattern = 'parallel'
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
        aggregated = this.aggregateHierarchical(results);
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
   * 先頭を親、以降を子として分離する（Orchestrator.executeHierarchical の順序と対応）
   */
  private aggregateHierarchical(results: TaskResult[]): any {
    if (results.length === 0) {
      return { parent: undefined, children: [] };
    }

    const [parentResult, ...childResults] = results;
    return {
      parent: {
        task_id: parentResult.taskId,
        result: parentResult.result,
        status: parentResult.status,
      },
      children: childResults.map((r) => ({
        task_id: r.taskId,
        result: r.result,
        status: r.status,
      })),
    };
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

