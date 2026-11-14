import { describe, it, expect, vi } from 'vitest';
import { ResearchAgent } from '../../src/agents/research/research-agent';
import type { Task } from '../../src/types/index';

const task = { id: 't1', type: 'research', parameters: { query: 'vitest' } } as unknown as Task;

const mkAgent = () =>
  new ResearchAgent({
    behavior: {
      retry_policy: { max_attempts: 2, backoff: 'fixed', initial_delay_ms: 1 },
      timeout: 1000,
      fallback: 'return_partial',
    },
  });

describe('ResearchAgent (mock tools)', () => {
  it('happy path: search + fetch を経て completed を返す', async () => {
    const agent = mkAgent();
    const search = vi.spyOn(agent.getTool('web_search')!, 'invoke').mockResolvedValue({
      results: [{ url: 'u1' }, { url: 'u2' }],
    });
    const fetch = vi
      .spyOn(agent.getTool('web_fetch')!, 'invoke')
      .mockImplementation(async (p) => ({ url: p.url, content: 'body-' + p.url }));

    const res = await agent.execute(task);

    expect(res.status).toBe('completed');
    expect(search).toHaveBeenCalledTimes(1);
    expect(fetch).toHaveBeenCalledTimes(2);
    expect(res.result.summary).toContain('body-u1');
    expect(res.result.sources).toHaveLength(2);
    expect(res.metrics?.tools_called).toBe(3);
    expect(agent.getState().status).toBe('completed');
  });

  it('failure: web_search が例外を投げ続けると failed を記録して throw する', async () => {
    const agent = mkAgent();
    const search = vi
      .spyOn(agent.getTool('web_search')!, 'invoke')
      .mockRejectedValue(new Error('search down'));

    await expect(agent.execute(task)).rejects.toThrow('search down');

    expect(search).toHaveBeenCalledTimes(2); // retry_policy.max_attempts
    const state = agent.getState();
    expect(state.status).toBe('failed');
    expect(state.currentTask).toBeNull();
    expect(state.context.history[0].status).toBe('failed');
    expect(state.context.history[0].error?.code).toBe('RESEARCH_ERROR');
  });
});
