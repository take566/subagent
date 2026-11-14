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
