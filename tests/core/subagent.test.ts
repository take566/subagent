import { describe, it, expect } from 'vitest';
import { Subagent } from '../../src/core/subagent.js';
import { ErrorHandler } from '../../src/utils/error-handler.js';
import { Logger } from '../../src/utils/logger.js';
import type { SubagentConfig, Task, TaskResult, Tool } from '../../src/types/index.js';

const tool = (name: string, fn: () => Promise<any>): Tool =>
  ({ name, description: name, parameters: {}, invoke: fn }) as Tool;

class TestAgent extends Subagent {
  async execute(_task: Task): Promise<TaskResult> {
    return { taskId: 't', status: 'completed' };
  }
  call(name: string) {
    return this.callTool(name, {});
  }
  finalize(r: TaskResult) {
    return this.applyEscalation(r);
  }
}

const config = { id: 'a', name: 'a', version: '1', tools: ['p', 'b'] } as unknown as SubagentConfig;
const handler = () =>
  new ErrorHandler(
    { tool_failure: { alternatives: [{ primary: 'p', backup: 'b' }] } } as any,
    new Logger('t')
  );
const fail = (m: string) => async () => {
  throw new Error(m);
};

describe('Subagent + ErrorHandler', () => {
  it('retries once with the alternative tool', async () => {
    const a = new TestAgent(config, [tool('p', fail('p')), tool('b', async () => 'ok')]);
    a.setErrorHandler(handler());
    expect(await a.call('p')).toBe('ok');
  });

  it('throws the original error when the alternative also fails', async () => {
    const a = new TestAgent(config, [tool('p', fail('p-err')), tool('b', fail('b-err'))]);
    a.setErrorHandler(handler());
    await expect(a.call('p')).rejects.toThrow('p-err');
  });

  it('behaves as before without an ErrorHandler', async () => {
    const a = new TestAgent(config, [tool('p', fail('p-err')), tool('b', async () => 'ok')]);
    await expect(a.call('p')).rejects.toThrow('p-err');
  });

  it('sets escalate when a condition matches', () => {
    const a = new TestAgent(config);
    a.setErrorHandler(handler(), ['confidence < 0.5']);
    expect(a.finalize({ taskId: 't', status: 'completed', confidence: 0.1 }).escalate).toBe(true);
    expect(a.finalize({ taskId: 't', status: 'completed', confidence: 0.9 }).escalate).toBeUndefined();
  });
});
