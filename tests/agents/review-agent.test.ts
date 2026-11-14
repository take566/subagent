import { describe, it, expect, vi } from 'vitest';
import { ReviewAgent } from '../../src/agents/review/review-agent';
import type { Task } from '../../src/types/index';

const mk = (source_code: string) =>
  ({ id: 't4', type: 'review', parameters: { source_code } }) as unknown as Task;

describe('ReviewAgent (mock tools)', () => {
  it('happy path: view と lint の結果を統合し completed', async () => {
    const agent = new ReviewAgent();
    vi.spyOn(agent.getTool('view')!, 'invoke').mockResolvedValue({ content: '// ok\nconst a = 1;' });
    const lint = vi
      .spyOn(agent.getTool('bash_tool')!, 'invoke')
      .mockResolvedValue({ issues: [{ type: 'error', message: 'lint-issue' }] });

    const res = await agent.execute(mk('/src/a.ts'));

    expect(res.status).toBe('completed');
    expect(lint).toHaveBeenCalledTimes(1);
    expect(res.result.issues).toEqual([{ type: 'error', message: 'lint-issue' }]);
    expect(typeof res.result.score).toBe('number');
  });

  it('failure: view が例外を投げると failed を記録して throw する', async () => {
    const agent = new ReviewAgent();
    vi.spyOn(agent.getTool('view')!, 'invoke').mockRejectedValue(new Error('ENOENT'));
    const lint = vi.spyOn(agent.getTool('bash_tool')!, 'invoke');

    await expect(agent.execute(mk('/missing.ts'))).rejects.toThrow('ENOENT');

    expect(lint).not.toHaveBeenCalled();
    const state = agent.getState();
    expect(state.status).toBe('failed');
    expect(state.context.history[0].error?.code).toBe('REVIEW_ERROR');
  });

  it('lint が例外でもレビューは completed (lint は任意扱い)', async () => {
    const agent = new ReviewAgent();
    vi.spyOn(agent.getTool('bash_tool')!, 'invoke').mockRejectedValue(new Error('lint boom'));

    const res = await agent.execute(mk('const x = 1;'));

    expect(res.status).toBe('completed');
    expect(res.result.issues).toEqual([]);
  });
});
