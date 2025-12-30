# Subagent 設計ドキュメント

## 1. 概要

Subagentは、メインエージェント（Orchestrator）から特定のタスクを委譲され、専門的な処理を実行する自律的なコンポーネントです。複雑なワークフローを分割し、並列処理や専門性の分離を実現します。

---

## 2. アーキテクチャ

```
┌─────────────────────────────────────────────────────────────┐
│                     Orchestrator Agent                       │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐  │
│  │ Task Parser │  │  Planner    │  │ Result Aggregator   │  │
│  └─────────────┘  └─────────────┘  └─────────────────────┘  │
└───────────────────────────┬─────────────────────────────────┘
                            │ A2A Protocol
        ┌───────────────────┼───────────────────┐
        ▼                   ▼                   ▼
┌───────────────┐   ┌───────────────┐   ┌───────────────┐
│  Subagent A   │   │  Subagent B   │   │  Subagent C   │
│  (Research)   │   │  (Code Gen)   │   │  (Review)     │
├───────────────┤   ├───────────────┤   ├───────────────┤
│ - Web Search  │   │ - File Create │   │ - Analysis    │
│ - Summarize   │   │ - Refactor    │   │ - Validation  │
│ - Extract     │   │ - Test Gen    │   │ - Feedback    │
└───────────────┘   └───────────────┘   └───────────────┘
```

---

## 3. Subagent 定義スキーマ

```yaml
subagent:
  id: "subagent-001"
  name: "ResearchAgent"
  version: "1.0.0"
  
  # 役割と専門性
  role:
    description: "情報収集と要約を担当"
    capabilities:
      - web_search
      - document_analysis
      - summarization
    constraints:
      - max_search_queries: 10
      - max_tokens_output: 4000
  
  # 入出力インターフェース
  interface:
    input:
      - name: "query"
        type: "string"
        required: true
      - name: "context"
        type: "object"
        required: false
    output:
      - name: "result"
        type: "object"
        schema:
          summary: "string"
          sources: "array"
          confidence: "number"
  
  # 利用可能なツール
  tools:
    - web_search
    - web_fetch
    - document_parser
  
  # 振る舞い設定
  behavior:
    retry_policy:
      max_attempts: 3
      backoff: "exponential"
    timeout: 60000
    fallback: "return_partial"
```

---

## 4. 通信プロトコル（A2A）

### 4.1 メッセージフォーマット

```json
{
  "message_id": "msg-uuid-001",
  "timestamp": "2025-12-28T10:00:00Z",
  "from": "orchestrator",
  "to": "subagent-research",
  "type": "task_request",
  "payload": {
    "task_id": "task-001",
    "action": "research",
    "parameters": {
      "query": "最新のCI/CDトレンド",
      "depth": "detailed"
    },
    "context": {
      "parent_task": "blog-article-generation",
      "deadline": "2025-12-28T11:00:00Z"
    }
  },
  "metadata": {
    "priority": "high",
    "correlation_id": "session-abc123"
  }
}
```

### 4.2 レスポンスフォーマット

```json
{
  "message_id": "msg-uuid-002",
  "in_reply_to": "msg-uuid-001",
  "timestamp": "2025-12-28T10:05:00Z",
  "from": "subagent-research",
  "to": "orchestrator",
  "type": "task_result",
  "status": "completed",
  "payload": {
    "task_id": "task-001",
    "result": {
      "summary": "...",
      "sources": [...],
      "confidence": 0.92
    }
  },
  "metrics": {
    "duration_ms": 5000,
    "tokens_used": 2500
  }
}
```

---

## 5. Subagent タイプ別設計

### 5.1 Research Agent

| 項目 | 内容 |
|------|------|
| **目的** | 情報収集・調査・要約 |
| **入力** | 検索クエリ、調査対象、深度指定 |
| **出力** | 構造化された調査結果、ソース一覧 |
| **ツール** | web_search, web_fetch, drive_search |

### 5.2 Code Generation Agent

| 項目 | 内容 |
|------|------|
| **目的** | コード生成・リファクタリング |
| **入力** | 仕様、言語、フレームワーク指定 |
| **出力** | 生成コード、テストコード |
| **ツール** | file_create, bash_tool, str_replace |

### 5.3 Review Agent

| 項目 | 内容 |
|------|------|
| **目的** | コードレビュー・品質チェック |
| **入力** | ソースコード、レビュー基準 |
| **出力** | 問題点リスト、改善提案、スコア |
| **ツール** | view, bash_tool (lint実行) |

### 5.4 Document Agent

| 項目 | 内容 |
|------|------|
| **目的** | ドキュメント作成・編集 |
| **入力** | コンテンツ、フォーマット指定 |
| **出力** | 完成ドキュメント（MD/DOCX/PDF） |
| **ツール** | file_create, Notion tools |

---

## 6. オーケストレーションパターン

### 6.1 Sequential（逐次実行）

```
Orchestrator → SubA → SubB → SubC → Result
```

依存関係がある場合に使用。前のSubagentの出力を次の入力として渡す。

### 6.2 Parallel（並列実行）

```
                ┌→ SubA ─┐
Orchestrator ───┼→ SubB ─┼→ Aggregate → Result
                └→ SubC ─┘
```

独立したタスクを同時実行し、最後に結果を統合。

### 6.3 Hierarchical（階層実行）

```
Orchestrator
    └→ SubA (Coordinator)
           ├→ SubA-1
           └→ SubA-2
```

Subagentが更にSubagentを制御する入れ子構造。

### 6.4 Conditional（条件分岐）

```
Orchestrator → Router → [条件判定]
                           ├─ (条件A) → SubA
                           └─ (条件B) → SubB
```

---

## 7. エラーハンドリング

```yaml
error_handling:
  strategies:
    timeout:
      action: "retry_with_backoff"
      max_retries: 3
      fallback: "delegate_to_alternative"
    
    validation_error:
      action: "request_clarification"
      escalate_after: 2
    
    tool_failure:
      action: "use_alternative_tool"
      alternatives:
        - primary: "web_search"
          backup: "drive_search"
    
    partial_failure:
      action: "return_partial_with_flag"
      notify: true

  escalation:
    path: "subagent → orchestrator → human"
    conditions:
      - "confidence < 0.5"
      - "critical_error"
      - "requires_approval"
```

---

## 8. 状態管理

```typescript
interface SubagentState {
  id: string;
  status: 'idle' | 'running' | 'waiting' | 'completed' | 'failed';
  currentTask: Task | null;
  context: {
    memory: Map<string, any>;      // 短期記憶
    history: TaskResult[];          // 実行履歴
    sharedContext: SharedContext;   // 他Agentと共有
  };
  metrics: {
    tasksCompleted: number;
    avgDuration: number;
    successRate: number;
  };
}
```

---

## 9. 実装例（TypeScript）

```typescript
abstract class Subagent {
  protected id: string;
  protected tools: Tool[];
  protected state: SubagentState;

  abstract async execute(task: Task): Promise<TaskResult>;

  protected async callTool(name: string, params: object): Promise<any> {
    const tool = this.tools.find(t => t.name === name);
    if (!tool) throw new Error(`Tool ${name} not available`);
    return await tool.invoke(params);
  }

  protected async reportProgress(progress: number, message: string): void {
    await this.emit('progress', { taskId: this.state.currentTask?.id, progress, message });
  }
}

class ResearchSubagent extends Subagent {
  async execute(task: Task): Promise<TaskResult> {
    this.state.status = 'running';
    
    // 1. 検索実行
    const searchResults = await this.callTool('web_search', { 
      query: task.parameters.query 
    });
    await this.reportProgress(0.3, 'Search completed');
    
    // 2. 詳細取得
    const details = await Promise.all(
      searchResults.slice(0, 3).map(r => 
        this.callTool('web_fetch', { url: r.url })
      )
    );
    await this.reportProgress(0.7, 'Details fetched');
    
    // 3. 要約生成
    const summary = await this.synthesize(details);
    
    return {
      taskId: task.id,
      status: 'completed',
      result: { summary, sources: searchResults },
      confidence: this.calculateConfidence(details)
    };
  }
}
```

---

## 10. MCP統合設計

```json
{
  "mcpServers": {
    "subagent-research": {
      "command": "node",
      "args": ["./agents/research/server.js"],
      "env": {
        "AGENT_ID": "research-001",
        "MAX_CONCURRENT": "5"
      }
    },
    "subagent-codegen": {
      "command": "node",
      "args": ["./agents/codegen/server.js"]
    }
  }
}
```

---

## 11. 監視・ログ

```yaml
observability:
  logging:
    level: "info"
    format: "json"
    fields:
      - agent_id
      - task_id
      - correlation_id
      - duration
      - status

  metrics:
    - name: "subagent_task_duration"
      type: "histogram"
      labels: ["agent_type", "task_type"]
    
    - name: "subagent_success_rate"
      type: "gauge"
      labels: ["agent_id"]

  tracing:
    enabled: true
    propagation: "w3c"
    sample_rate: 0.1
```

---

## 12. セキュリティ考慮事項

| カテゴリ | 対策 |
|---------|------|
| **認証** | Agent間通信にJWT/mTLS使用 |
| **認可** | 各Subagentにツール使用権限を明示的に付与 |
| **入力検証** | すべての入力をスキーマでバリデーション |
| **出力サニタイズ** | コード実行結果の検証、機密情報マスキング |
| **レート制限** | Subagentごとにリソース使用量を制限 |

---

## 付録A: 用語集

| 用語 | 説明 |
|------|------|
| **Orchestrator** | Subagentを統括し、タスクの分配と結果統合を行うメインエージェント |
| **Subagent** | 特定のタスクに特化した自律的なエージェントコンポーネント |
| **A2A Protocol** | Agent-to-Agent通信プロトコル |
| **MCP** | Model Context Protocol - エージェントとツール間の標準化プロトコル |
| **Task** | Subagentに委譲される作業単位 |
| **Context** | タスク実行に必要な背景情報・状態 |

---

## 付録B: 関連ドキュメント

- A2A通信プロトコル仕様書
- MCP統合ガイドライン
- エラーハンドリングベストプラクティス
- Subagentデプロイメントガイド

---

*Document Version: 1.0.0*  
*Last Updated: 2025-12-30*
