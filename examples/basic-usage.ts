/**
 * 基本的な使用例
 */

import { createDefaultOrchestrator } from '../src/index.js';

async function main() {
  const orchestrator = createDefaultOrchestrator();

  console.log('=== Research Agent の使用例 ===');
  const researchResult = await orchestrator.execute({
    id: 'task-research-001',
    action: 'research',
    parameters: {
      query: '最新のCI/CDトレンド',
      depth: 'detailed',
    },
  });
  console.log('Research Result:', JSON.stringify(researchResult, null, 2));

  console.log('\n=== 並列実行の例 ===');
  const parallelResult = await orchestrator.execute(
    [
      {
        id: 'task-1',
        action: 'research',
        parameters: { query: 'TypeScript best practices' },
      },
      {
        id: 'task-2',
        action: 'document',
        parameters: {
          content: '# TypeScript Best Practices\n\n...',
          format: 'markdown',
        },
      },
    ],
    'parallel'
  );
  console.log('Parallel Result:', JSON.stringify(parallelResult, null, 2));

  console.log('\n=== 逐次実行の例 ===');
  const sequentialResult = await orchestrator.execute(
    [
      {
        id: 'task-seq-1',
        action: 'research',
        parameters: { query: 'API design patterns' },
      },
      {
        id: 'task-seq-2',
        action: 'codegen',
        parameters: {
          specification: 'Implement API client based on research',
          language: 'typescript',
        },
        context: {
          parent_task: 'task-seq-1',
        },
      },
    ],
    'sequential'
  );
  console.log('Sequential Result:', JSON.stringify(sequentialResult, null, 2));
}

main().catch(console.error);

