import { describe, it, expect } from 'vitest';
import { ResultAggregator } from './result-aggregator.js';
import type { TaskResult } from '../types/index.js';

const r = (taskId: string, result: any): TaskResult => ({
  taskId,
  status: 'completed',
  result,
});

describe('ResultAggregator hierarchical', () => {
  const agg = new ResultAggregator();

  it('separates parent from children using dependencies', () => {
    // 親が先頭でなくても依存を持たない結果が親になる
    const results = [r('c1', 'C1'), r('p', 'P'), r('c2', 'C2')];
    const deps = new Map([
      ['c1', ['p']],
      ['c2', ['p']],
    ]);
    const { aggregated } = agg.aggregate(results, 'hierarchical', deps);
    expect(aggregated.parent).toBe('P');
    expect(aggregated.parent_task_id).toBe('p');
    expect(aggregated.children.map((c: any) => c.task_id)).toEqual(['c1', 'c2']);
  });

  it('nests grandchildren under their dependency', () => {
    const results = [r('p', 'P'), r('c', 'C'), r('g', 'G')];
    const deps = new Map([
      ['c', ['p']],
      ['g', ['c']],
    ]);
    const { aggregated } = agg.aggregate(results, 'hierarchical', deps);
    expect(aggregated.children).toHaveLength(1);
    expect(aggregated.children[0].task_id).toBe('c');
    expect(aggregated.children[0].children[0]).toMatchObject({
      task_id: 'g',
      result: 'G',
    });
  });

  it('falls back to first result as parent without dependencies', () => {
    const { aggregated } = agg.aggregate(
      [r('p', 'P'), r('c', 'C')],
      'hierarchical'
    );
    expect(aggregated.parent).toBe('P');
    expect(aggregated.children.map((c: any) => c.task_id)).toEqual(['c']);
  });

  it('attaches cyclic children to the parent without looping', () => {
    const deps = new Map([
      ['a', ['b']],
      ['b', ['a']],
    ]);
    const { aggregated } = agg.aggregate(
      [r('p', 'P'), r('a', 'A'), r('b', 'B')],
      'hierarchical',
      deps
    );
    expect(aggregated.parent).toBe('P');
    expect(aggregated.children.map((c: any) => c.task_id)).toEqual(['a', 'b']);
  });
});

describe('ResultAggregator other patterns', () => {
  const agg = new ResultAggregator();

  it('sequential returns the last result and the chain', () => {
    const { aggregated, success } = agg.aggregate([r('a', 'A'), r('b', 'B')], 'sequential');
    expect(success).toBe(true);
    expect(aggregated.final_result).toBe('B');
    expect(aggregated.chain.map((c: any) => c.task_id)).toEqual(['a', 'b']);
  });

  it('parallel merges object results', () => {
    const { aggregated } = agg.aggregate([r('a', { x: 1 }), r('b', { y: 2 })], 'parallel');
    expect(aggregated.combined).toEqual({ x: 1, y: 2 });
    expect(aggregated.results).toHaveLength(2);
  });

  it('conditional lists the executed tasks', () => {
    const { aggregated } = agg.aggregate([r('a', 'A')], 'conditional');
    expect(aggregated.executed).toEqual([{ task_id: 'a', result: 'A', status: 'completed' }]);
  });

  it('computes metrics including failures', () => {
    const { success, metrics } = agg.aggregate([
      { taskId: 'a', status: 'completed', confidence: 0.5, metrics: { duration_ms: 10 } as any },
      { taskId: 'b', status: 'failed', confidence: 1, metrics: { duration_ms: 5 } as any },
      { taskId: 'c', status: 'completed' },
    ]);
    expect(success).toBe(false);
    expect(metrics).toEqual({
      total_duration_ms: 15,
      success_count: 2,
      failure_count: 1,
      avg_confidence: 0.75,
    });
  });

  it('hierarchical with no results yields no parent', () => {
    const { aggregated } = agg.aggregate([], 'hierarchical', new Map());
    expect(aggregated).toEqual({ parent: undefined, children: [] });
  });
});
