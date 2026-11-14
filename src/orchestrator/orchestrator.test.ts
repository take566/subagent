import { EventEmitter } from 'events';
import { describe, it, expect } from 'vitest';
import { Orchestrator } from './orchestrator.js';

function fakeAgent(id: string, capabilities: string[]): any {
  const agent: any = new EventEmitter();
  agent.getConfig = () => ({ id, role: { capabilities } });
  agent.execute = async () => ({ agent: id });
  return agent;
}

describe('Orchestrator.selectSubagent', () => {
  it('selects by capability mapped from action', () => {
    const o = new Orchestrator();
    o.registerSubagent(fakeAgent('a', ['code_generation']));
    o.registerSubagent(fakeAgent('b', ['web_search']));
    const picked = (o as any).selectSubagent({ id: 't', action: 'research' });
    expect(picked.getConfig().id).toBe('b');
  });

  it('throws a clear error when no capability is registered', () => {
    const o = new Orchestrator();
    o.registerSubagent(fakeAgent('a', ['code_generation']));
    expect(() => (o as any).selectSubagent({ id: 't', action: 'review' })).toThrow(
      /No subagent registered with capability "code_analysis"/
    );
  });
});
