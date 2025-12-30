/**
 * Subagentシステム メインエントリーポイント
 */

export * from './types/index.js';
export * from './core/subagent.js';
export * from './core/tool.js';
export * from './protocol/a2a.js';
export * from './orchestrator/orchestrator.js';
export * from './orchestrator/task-parser.js';
export * from './orchestrator/planner.js';
export * from './orchestrator/result-aggregator.js';
export * from './agents/research/research-agent.js';
export * from './agents/codegen/codegen-agent.js';
export * from './agents/review/review-agent.js';
export * from './agents/document/document-agent.js';
export * from './utils/logger.js';
export * from './utils/error-handler.js';
export * from './utils/retry.js';

// 使用例
import { Orchestrator } from './orchestrator/orchestrator.js';
import { ResearchAgent } from './agents/research/research-agent.js';
import { CodeGenAgent } from './agents/codegen/codegen-agent.js';
import { ReviewAgent } from './agents/review/review-agent.js';
import { DocumentAgent } from './agents/document/document-agent.js';

/**
 * デフォルトのOrchestratorインスタンスを作成
 */
export function createDefaultOrchestrator(): Orchestrator {
  const orchestrator = new Orchestrator();

  // デフォルトのSubagentを登録
  orchestrator.registerSubagent(new ResearchAgent());
  orchestrator.registerSubagent(new CodeGenAgent());
  orchestrator.registerSubagent(new ReviewAgent());
  orchestrator.registerSubagent(new DocumentAgent());

  return orchestrator;
}

