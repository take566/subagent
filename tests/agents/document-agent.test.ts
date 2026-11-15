import { describe, it, expect, vi } from 'vitest';
import { DocumentAgent } from '../../src/agents/document/document-agent';
import type { Task } from '../../src/types/index';

const task = {
  id: 't3',
  type: 'document',
  parameters: { content: '# Title', format: 'markdown' },
} as unknown as Task;

describe('DocumentAgent (mock tools)', () => {
  it('happy path: file_create の結果パスを返し completed', async () => {
    const agent = new DocumentAgent();
    const create = vi
      .spyOn(agent.getTool('file_create')!, 'invoke')
      .mockResolvedValue({ path: '/tmp/doc.md', created: true });

    const res = await agent.execute(task);

    expect(res.status).toBe('completed');
    expect(create).toHaveBeenCalledTimes(1);
    expect(res.result.document_path).toBe('/tmp/doc.md');
    expect(res.result.format).toBe('markdown');
    expect(agent.getState().status).toBe('completed');
  });

  it('failure: file_create が例外を投げると failed を記録して throw する', async () => {
    const agent = new DocumentAgent();
    vi.spyOn(agent.getTool('file_create')!, 'invoke').mockRejectedValue(new Error('EACCES'));

    await expect(agent.execute(task)).rejects.toThrow('EACCES');

    const state = agent.getState();
    expect(state.status).toBe('failed');
    expect(state.currentTask).toBeNull();
    expect(state.context.history[0].status).toBe('failed');
  });
});
