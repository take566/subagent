import { describe, it, expect } from 'vitest';
import { ResultAggregator } from './result-aggregator.js';
import type { TaskResult } from '../types/index.js';

describe('ResultAggregator hierarchical', () => {
  it('separates parent and children', () => {
    const results: TaskResult[] = [
      { taskId: 'parent', status: 'completed', result: { role: 'parent' } },
      { taskId: 'child-1', status: 'completed', result: { n: 1 } },
      { taskId: 'child-2', status: 'failed', result: { n: 2 } },
    ];
    const agg = new ResultAggregator().aggregate(results, 'hierarchical');
    expect(agg.aggregated.parent.task_id).toBe('parent');
    expect(agg.aggregated.parent.result).toEqual({ role: 'parent' });
    expect(agg.aggregated.children).toHaveLength(2);
    expect(agg.aggregated.children[0].task_id).toBe('child-1');
    expect(agg.success).toBe(false);
    expect(agg.metrics.failure_count).toBe(1);
  });

  it('handles empty results', () => {
    const agg = new ResultAggregator().aggregate([], 'hierarchical');
    expect(agg.aggregated.parent).toBeUndefined();
    expect(agg.aggregated.children).toEqual([]);
  });
});
