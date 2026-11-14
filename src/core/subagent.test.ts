import { describe, it, expect } from 'vitest';
import { Subagent } from './subagent.js';
import type { Task, TaskResult, SubagentConfig, Tool } from '../types/index.js';

const config = (timeout?: number): SubagentConfig =>
  ({
    id: 't',
    name: 't',
    version: '1',
    tools: ['slow'],
    behavior: { timeout },
  }) as unknown as SubagentConfig;

let toolSignal: AbortSignal | undefined;
const slowTool: Tool = {
  name: 'slow',
  description: '',
  parameters: {},
  invoke: (_p, signal) => {
    toolSignal = signal;
    return new Promise((_, reject) => {
      signal?.addEventListener('abort', () => reject(signal.reason));
    });
  },
};

class Slow extends Subagent {
  async execute(task: Task): Promise<TaskResult> {
    await this.callTool('slow', {});
    return { taskId: task.id, status: 'completed' };
  }
}

describe('executeWithTimeout', () => {
  it('timeout 超過で failed + TIMEOUT を返し、ツールの signal が abort される', async () => {
    const agent = new Slow(config(30), [slowTool]);
    const r = await agent.executeWithTimeout({ id: 'a' } as Task);
    expect(r.status).toBe('failed');
    expect(r.error?.code).toBe('TIMEOUT');
    expect(toolSignal?.aborted).toBe(true);
  });

  it('timeout 内に完了すれば結果をそのまま返す', async () => {
    class Fast extends Subagent {
      async execute(task: Task): Promise<TaskResult> {
        return { taskId: task.id, status: 'completed' };
      }
    }
    const r = await new Fast(config(1000)).executeWithTimeout({ id: 'b' } as Task);
    expect(r.status).toBe('completed');
  });
});
