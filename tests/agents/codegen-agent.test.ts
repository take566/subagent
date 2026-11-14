import { describe, it, expect, vi } from 'vitest';
import { CodeGenAgent } from '../../src/agents/codegen/codegen-agent';
import type { Task } from '../../src/types/index';

const task = { id: 't2', type: 'codegen', parameters: { specification: 'hello' } } as unknown as Task;

describe('CodeGenAgent (mock tools)', () => {
  it('happy path: file_create を本体とテスト分だけ呼び completed', async () => {
    const agent = new CodeGenAgent();
    const create = vi
      .spyOn(agent.getTool('file_create')!, 'invoke')
      .mockImplementation(async (p) => ({ path: p.path, created: true }));

    const res = await agent.execute(task);

    expect(res.status).toBe('completed');
    expect(create).toHaveBeenCalledTimes(2);
    expect(res.result.files).toHaveLength(1);
    expect(res.result.test_files).toHaveLength(1);
    expect(agent.getState().status).toBe('completed');
  });

  it('failure: file_create が例外を投げると failed を記録して throw する', async () => {
    const agent = new CodeGenAgent();
    vi.spyOn(agent.getTool('file_create')!, 'invoke').mockRejectedValue(new Error('disk full'));

    await expect(agent.execute(task)).rejects.toThrow('disk full');

    const state = agent.getState();
    expect(state.status).toBe('failed');
    expect(state.context.history[0].status).toBe('failed');
    expect(state.context.history[0].error?.code).toBe('CODEGEN_ERROR');
  });
});
