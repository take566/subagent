import { describe, it, expect } from 'vitest';
import { Planner } from './planner.js';
import type { Task } from '../types/index.js';

const t = (id: string, extra: Partial<Task> = {}): Task => ({
  id,
  action: 'research',
  parameters: {},
  ...extra,
});

describe('Planner', () => {
  const planner = new Planner();

  describe('pattern detection', () => {
    it('detects sequential when a task has a parent', () => {
      const plan = planner.createPlan([t('a'), t('b', { context: { parent_task: 'a' } })]);
      expect(plan.pattern).toBe('sequential');
    });

    it('detects conditional when a task has a condition', () => {
      const plan = planner.createPlan([t('a', { parameters: { condition: 'previous.success' } })]);
      expect(plan.pattern).toBe('conditional');
    });

    it('defaults to parallel', () => {
      expect(planner.createPlan([t('a'), t('b')]).pattern).toBe('parallel');
    });
  });

  describe('sequential / topological sort', () => {
    it('orders dependencies before dependents', () => {
      // c depends on b, b depends on a; given in reverse order
      const tasks = [
        t('c', { context: { parent_task: 'b' } }),
        t('b', { context: { parent_task: 'a' } }),
        t('a'),
      ];
      const plan = planner.createPlan(tasks, 'sequential');
      expect(plan.tasks.map((x) => x.id)).toEqual(['a', 'b', 'c']);
      expect(plan.dependencies?.get('b')).toEqual(['a']);
      expect(plan.dependencies?.get('c')).toEqual(['b']);
      expect(plan.dependencies?.has('a')).toBe(false);
    });

    it('keeps all tasks when a cycle exists and does not loop forever', () => {
      const tasks = [
        t('a', { context: { parent_task: 'b' } }),
        t('b', { context: { parent_task: 'a' } }),
      ];
      const plan = planner.createPlan(tasks, 'sequential');
      expect(plan.tasks.map((x) => x.id).sort()).toEqual(['a', 'b']);
    });

    it('ignores dependencies on unknown tasks', () => {
      const plan = planner.createPlan([t('a', { context: { parent_task: 'zzz' } })], 'sequential');
      expect(plan.tasks.map((x) => x.id)).toEqual(['a']);
    });
  });

  it('parallel plan keeps tasks without dependencies', () => {
    const plan = planner.createPlan([t('a'), t('b')], 'parallel');
    expect(plan.pattern).toBe('parallel');
    expect(plan.tasks).toHaveLength(2);
    expect(plan.dependencies).toBeUndefined();
  });

  it('hierarchical plan links children to the parent', () => {
    const plan = planner.createPlan(
      [t('p'), t('c1', { context: { parent_task: 'p' } }), t('c2', { context: { parent_task: 'p' } })],
      'hierarchical'
    );
    expect(plan.pattern).toBe('hierarchical');
    expect(plan.dependencies?.get('c1')).toEqual(['p']);
    expect(plan.dependencies?.get('c2')).toEqual(['p']);
    expect(plan.dependencies?.has('p')).toBe(false);
  });

  it('conditional plan collects rules from tasks with conditions', () => {
    const plan = planner.createPlan(
      [
        t('a'),
        t('b', { action: 'review', parameters: { condition: 'previous.success', target_subagent: 'rv' } }),
        t('c', { parameters: { condition: 'x' } }),
      ],
      'conditional'
    );
    expect(plan.conditions).toEqual([
      { condition: 'previous.success', target_subagent: 'rv', action: 'review' },
      { condition: 'x', target_subagent: 'default', action: 'research' },
    ]);
  });
});
