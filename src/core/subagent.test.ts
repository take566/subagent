import { describe, it, expect } from 'vitest';
import { Subagent, TimeoutError } from './subagent.js';
import type { Task, TaskResult, SubagentConfig } from '../types/index.js';

class SlowAgent extends Subagent {
  async execute(task: Task): Promise<TaskResult> {
    this.state.currentTask = task;
    await new Promise((r) => setTimeout(r, 200));
    return { taskId: task.id, status: 'completed', result: { ok: true } };
  }
}

const baseConfig: SubagentConfig = {
  id: 'slow-001',
  name: 'SlowAgent',
  version: '1.0.0',
  role: { description: 'slow', capabilities: ['slow'] },
  interface: {
    input: [{ name: 'x', type: 'string', required: true }],
    output: [{ name: 'result', type: 'object', required: true }],
  },
  tools: [],
  behavior: { timeout: 50 },
};

describe('Subagent.run', () => {
  it('returns validation failure without throwing', async () => {
    const agent = new SlowAgent(baseConfig);
    const result = await agent.run({
      id: 't1',
      action: 'slow',
      parameters: {},
    });
    expect(result.status).toBe('failed');
    expect(result.error?.code).toBe('VALIDATION_ERROR');
  });

  it('enforces behavior.timeout', async () => {
    const agent = new SlowAgent(baseConfig);
    const result = await agent.run({
      id: 't2',
      action: 'slow',
      parameters: { x: 'hi' },
    });
    expect(result.status).toBe('failed');
    expect(result.error?.code).toBe('TIMEOUT');
  });

  it('completes when under timeout', async () => {
    const agent = new SlowAgent({
      ...baseConfig,
      behavior: { timeout: 1000 },
    });
    const result = await agent.run({
      id: 't3',
      action: 'slow',
      parameters: { x: 'hi' },
    });
    expect(result.status).toBe('completed');
  });

  it('TimeoutError has code', () => {
    const err = new TimeoutError(10, 't');
    expect(err.code).toBe('TIMEOUT');
  });
});
