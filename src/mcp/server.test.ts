import { describe, it, expect } from 'vitest';
import { MCPServer } from './server.js';
import type { Subagent } from '../core/subagent.js';

function fakeAgent() {
  let id = 'orig';
  const state = { running: 0, peak: 0 };
  const agent = {
    setId: (v: string) => {
      id = v;
    },
    getConfig: () => ({ id, name: 'n', version: '1' }),
    getAvailableTools: () => [],
    getTool: () => undefined,
    execute: async () => {
      state.running++;
      state.peak = Math.max(state.peak, state.running);
      await new Promise((r) => setTimeout(r, 10));
      state.running--;
      return { ok: true };
    },
  };
  return { agent: agent as unknown as Subagent, state, getId: () => id };
}

async function callExecute(server: MCPServer) {
  const handlers = (server as any).server._requestHandlers as Map<string, any>;
  const handler = handlers.get('tools/call');
  return handler(
    { method: 'tools/call', params: { name: 'execute', arguments: { action: 'x' } } },
    {}
  );
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
