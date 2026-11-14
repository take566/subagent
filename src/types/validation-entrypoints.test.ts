import { describe, it, expect } from 'vitest';
import { ValidationError } from './validation.js';
import { Subagent } from '../core/subagent.js';
import { Orchestrator } from '../orchestrator/orchestrator.js';
import { ResearchAgent } from '../agents/research/research-agent.js';
import { CodeGenAgent } from '../agents/codegen/codegen-agent.js';
import { ReviewAgent } from '../agents/review/review-agent.js';
import { DocumentAgent } from '../agents/document/document-agent.js';

const bad = { action: 'x', parameters: {} } as any; // id 欠落

describe('validation entrypoints', () => {
  const agents = [
    ['ResearchAgent', () => new ResearchAgent()],
    ['CodeGenAgent', () => new CodeGenAgent()],
    ['ReviewAgent', () => new ReviewAgent()],
    ['DocumentAgent', () => new DocumentAgent()],
  ] as const;

  for (const [name, make] of agents) {
    it(`${name}: 実 config が検証を通り、execute 入口で不正 Task を拒否する`, async () => {
      const agent = make();
      await expect(agent.execute(bad)).rejects.toBeInstanceOf(ValidationError);
    });
  }

  it('Subagent コンストラクタで不正 config を拒否する', () => {
    class Dummy extends Subagent {
      async execute(): Promise<any> {
        return {};
      }
    }
    expect(() => new Dummy({ id: '' } as any)).toThrow(ValidationError);
  });

  it('Orchestrator.executeTask 入口で不正 Task を拒否し activeTasks を汚さない', async () => {
    const o = new Orchestrator() as any;
    await expect(o.executeTask(bad)).rejects.toBeInstanceOf(ValidationError);
    expect(o.activeTasks.size).toBe(0);
  });
});
