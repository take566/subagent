# Changelog

## [1.1.0] - 2026-09-02

### 追加
- SubagentConfig / Task の zod 実行時バリデーション (`src/types/validation.ts`)
- A2A メッセージ厳密スキーマと型ガード (`src/protocol/a2a-schema.ts`)
- `Subagent.run()` — バリデーション + `behavior.timeout` (AbortSignal)
- vitest ユニットテスト一式

### 修正
- ResultAggregator hierarchical の親/子分離ロジック

## [1.0.0] - 2025-12-30

### 追加
- Subagentシステムの基本実装
- Research Agent実装
- CodeGen Agent実装
- Review Agent実装
- Document Agent実装
- Orchestrator実装（Task Parser, Planner, Result Aggregator）
- A2A通信プロトコル実装
- エラーハンドリングとリトライ機構
- ログ・監視システム
- MCP統合
- 使用例とドキュメント

### 機能
- Sequential, Parallel, Hierarchical, Conditional実行パターン
- タスクの依存関係管理
- 結果の集約と統合
- プログレスレポート
- メトリクス収集
