import { describe, it, expect } from 'vitest';
import { EventEmitter } from 'events';
import { Orchestrator } from './orchestrator.js';
import type { Task, TaskResult } from '../types/index.js';
import type { Subagent } from '../core/subagent.js';

class FakeAgent extends EventEmitter {
  executed: Task[] = [];
  constructor(
    private id: string,
    private log: string[],
    private status: TaskResult['status'] = 'completed'
  ) {
    super();
  }
  getConfig() {
    return { id: this.id, name: this.id };
  }
  async execute(task: Task): Promise<TaskResult> {
    this.executed.push(task);
    this.log.push(task.id);
    return { taskId: task.id, status: this.status, result: { from: task.id } };
  }
}

const setup = (status: TaskResult['status'] = 'completed') => {
  const log: string[] = [];
  const agent = new FakeAgent('research-001', log, status);
  const orch = new Orchestrator();
  orch.registerSubagent(agent as unknown as Subagent);
  return { orch, agent, log };
};

const t = (id: string, extra: Partial<Task> = {}): Task => ({
  id,
  action: 'research',
  parameters: {},
  ...extra,
});

describe('Orchestrator', () => {
  it('registers and retrieves subagents', () => {
    const { orch, agent } = setup();
    expect(orch.getSubagent('research-001')).toBe(agent);
    expect(orch.getSubagent('nope')).toBeUndefined();
  });

  it('forwards subagent progress events', () => {
    const { orch, agent } = setup();
    const got: any[] = [];
    orch.on('subagent_progress', (e) => got.push(e));
    agent.emit('progress', { pct: 50 });
    expect(got).toEqual([{ agentId: 'research-001', pct: 50 }]);
  });

  it('sequential: runs in dependency order and passes previous results', async () => {
    const { orch, agent, log } = setup();
    const res = await orch.execute(
      [t('b', { context: { parent_task: 'a' } }), t('a')],
      'sequential'
    );
    expect(log).toEqual(['a', 'b']);
    expect(res.success).toBe(true);
    const b = agent.executed.find((x) => x.id === 'b')!;
    expect((b.context as any).previous_results[0].taskId).toBe('a');
    expect(res.aggregated.final_result).toEqual({ from: 'b' });
  });

  it('parallel: executes all tasks', async () => {
    const { orch, log } = setup();
    const res = await orch.execute([t('a'), t('b'), t('c')], 'parallel');
    expect([...log].sort()).toEqual(['a', 'b', 'c']);
    expect(res.metrics.success_count).toBe(3);
    expect(res.aggregated.combined).toEqual({ from: 'c' });
  });

  it('hierarchical: parent first, children receive parent_result', async () => {
    const { orch, agent, log } = setup();
    const res = await orch.execute(
      [t('p'), t('c1', { context: { parent_task: 'p' } }), t('c2', { context: { parent_task: 'p' } })],
      'hierarchical'
    );
    expect(log[0]).toBe('p');
    expect(log.slice(1).sort()).toEqual(['c1', 'c2']);
    expect((agent.executed.find((x) => x.id === 'c1')!.context as any).parent_result).toEqual({ from: 'p' });
    expect(res.aggregated.parent_task_id).toBe('p');
    expect(res.aggregated.children.map((c: any) => c.task_id)).toEqual(['c1', 'c2']);
  });

  it('conditional: skips tasks whose condition is false', async () => {
    const { orch, log } = setup('failed');
    const res = await orch.execute(
      [t('a'), t('b', { parameters: { condition: 'previous.success' } })],
      'conditional'
    );
    // a fails, so previous.success is false and b is skipped
    expect(log).toEqual(['a']);
    expect(res.success).toBe(false);
    expect(res.aggregated.executed).toHaveLength(1);
  });

  it('conditional: evaluates previous.results.length comparisons', async () => {
    const { orch, log } = setup();
    await orch.execute(
      [
        t('a'),
        t('b', { parameters: { condition: 'previous.results.length >= 1' } }),
        t('c', { parameters: { condition: 'previous.results.length > 5' } }),
      ],
      'conditional'
    );
    expect(log).toEqual(['a', 'b']);
  });

  it('rejects when no subagent matches the action', async () => {
    const { orch } = setup();
    await expect(orch.execute(t('a', { action: 'unknown' }), 'parallel')).rejects.toThrow(
      /No suitable subagent/
    );
  });

  it('returns an error response for an invalid A2A message', async () => {
    const { orch } = setup();
    const res: any = await orch.handleA2AMessage({ from: 'x' } as any);
    expect(res.type).toBe('error');
  });
});
