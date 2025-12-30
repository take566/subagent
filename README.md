# Subagent System

Subagentシステムは、メインエージェント（Orchestrator）から特定のタスクを委譲され、専門的な処理を実行する自律的なコンポーネントです。

## 機能

- **複数のSubagentタイプ**: Research, CodeGen, Review, Document
- **A2A通信プロトコル**: Agent-to-Agent通信の標準化
- **オーケストレーションパターン**: Sequential, Parallel, Hierarchical, Conditional
- **エラーハンドリング**: リトライ、フォールバック、エスカレーション
- **MCP統合**: Model Context Protocolによるツール統合
- **ログ・監視**: 構造化ログとメトリクス収集

## インストール

```bash
npm install
```

## ビルド

```bash
npm run build
```

## 使用方法

### 基本的な使用例

```typescript
import { createDefaultOrchestrator } from './src/index.js';

const orchestrator = createDefaultOrchestrator();

// タスクを実行
const result = await orchestrator.execute({
  id: 'task-001',
  action: 'research',
  parameters: {
    query: '最新のCI/CDトレンド',
    depth: 'detailed',
  },
});

console.log(result);
```

### 並列実行

```typescript
const tasks = [
  {
    id: 'task-1',
    action: 'research',
    parameters: { query: 'TypeScript best practices' },
  },
  {
    id: 'task-2',
    action: 'codegen',
    parameters: { specification: 'Create a utility function' },
  },
];

const result = await orchestrator.execute(tasks, 'parallel');
```

### 逐次実行

```typescript
const tasks = [
  {
    id: 'task-1',
    action: 'research',
    parameters: { query: 'API design patterns' },
  },
  {
    id: 'task-2',
    action: 'codegen',
    parameters: { specification: 'Implement API client' },
    context: {
      parent_task: 'task-1',
    },
  },
];

const result = await orchestrator.execute(tasks, 'sequential');
```

## MCPサーバー

各SubagentはMCPサーバーとして起動できます：

```bash
# Research Agent
node dist/mcp/research-server.js

# CodeGen Agent
node dist/mcp/codegen-server.js

# Review Agent
node dist/mcp/review-server.js

# Document Agent
node dist/mcp/document-server.js
```

## プロジェクト構造

```
src/
├── agents/          # Subagent実装
│   ├── research/
│   ├── codegen/
│   ├── review/
│   └── document/
├── core/           # コアクラス
├── orchestrator/   # Orchestrator実装
├── protocol/       # A2Aプロトコル
├── utils/          # ユーティリティ
├── mcp/            # MCP統合
└── types/          # 型定義
```

## ライセンス

MIT

