import { describe, it, expect } from 'vitest';
import { MCPServer } from './server.js';
import type { Subagent } from '../core/subagent.js';
import type { Task } from '../types/index.js';

function fakeAgent() {
  let id = 'orig';
  const state = { running: 0, peak: 0, calls: [] as Task[] };
  const agent = {
    setId: (v: string) => {
      id = v;
    },
    getConfig: () => ({ id, name: 'n', version: '1' }),
    getAvailableTools: () => [],
    getTool: () => undefined,
    execute: async (task: Task) => {
      state.calls.push(task);
      state.running++;
      state.peak = Math.max(state.peak, state.running);
      await new Promise((r) => setTimeout(r, 10));
      state.running--;
      return { ok: true };
    },
  };
  return { agent: agent as unknown as Subagent, state, getId: () => id };
}

async function callExecute(
  server: MCPServer,
  args: Record<string, unknown> = { action: 'x' }
) {
  const handlers = (server as any).server._requestHandlers as Map<string, any>;
  const handler = handlers.get('tools/call');
  return handler({ method: 'tools/call', params: { name: 'execute', arguments: args } }, {});
}

describe('MCPServer env', () => {
  it('AGENT_ID を agent id に反映する', () => {
    const f = fakeAgent();
    new MCPServer(f.agent, { AGENT_ID: 'research-001' });
    expect(f.getId()).toBe('research-001');
  });

  it('AGENT_ID 未設定なら id を変えない', () => {
    const f = fakeAgent();
    new MCPServer(f.agent, {});
    expect(f.getId()).toBe('orig');
  });

  it('MAX_CONCURRENT で同時実行数を制限する', async () => {
    const f = fakeAgent();
    const server = new MCPServer(f.agent, { MAX_CONCURRENT: '2' });
    await Promise.all(Array.from({ length: 6 }, () => callExecute(server)));
    expect(f.state.peak).toBe(2);
  });

  it('不正な MAX_CONCURRENT は無制限', async () => {
    const f = fakeAgent();
    const server = new MCPServer(f.agent, { MAX_CONCURRENT: 'abc' });
    await Promise.all(Array.from({ length: 4 }, () => callExecute(server)));
    expect(f.state.peak).toBe(4);
  });
});

describe('MCPServer execute の引数検証', () => {
  const invalid: Array<[string, Record<string, unknown>, string]> = [
    ['action 欠落', {}, 'action'],
    ['action が数値', { action: 1 }, 'action'],
    ['parameters が配列', { action: 'x', parameters: [] }, 'parameters'],
    ['parameters が文字列', { action: 'x', parameters: 'a' }, 'parameters'],
    ['task_id が数値', { action: 'x', task_id: 1 }, 'task_id'],
  ];

  for (const [label, args, argName] of invalid) {
    it(`${label} はエラーを返し execute を呼ばない`, async () => {
      const f = fakeAgent();
      const server = new MCPServer(f.agent, {});
      const result = await callExecute(server, args);
      expect(result.isError).toBe(true);
      expect(result.content[0].text).toContain(argName);
      expect(f.state.calls).toHaveLength(0);
    });
  }

  it('正しい引数なら execute に渡す', async () => {
    const f = fakeAgent();
    const server = new MCPServer(f.agent, {});
    const result = await callExecute(server, { action: 'x', parameters: { a: 1 }, task_id: 't1' });
    expect(result.isError).toBeUndefined();
    expect(f.state.calls).toHaveLength(1);
    expect(f.state.calls[0]).toMatchObject({ id: 't1', action: 'x', parameters: { a: 1 } });
  });

  it('action のみでも実行できる', async () => {
    const f = fakeAgent();
    const server = new MCPServer(f.agent, {});
    const result = await callExecute(server, { action: 'x' });
    expect(result.isError).toBeUndefined();
    expect(f.state.calls).toHaveLength(1);
    expect(f.state.calls[0]).toMatchObject({ action: 'x', parameters: {} });
  });
});
