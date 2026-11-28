# Subagent System

Subagentシステムは、メインエージェント（Orchestrator）から特定のタスクを委譲され、専門的な処理を実行する自律的なコンポーネントです。Rust で実装しています（v1.1.0 で TypeScript から移植）。

## 機能

- **複数のSubagentタイプ**: Research, CodeGen, Review, Document
- **A2A通信プロトコル**: Agent-to-Agent通信の標準化
- **オーケストレーションパターン**: Sequential, Parallel, Hierarchical, Conditional
- **エラーハンドリング**: リトライ、フォールバック、エスカレーション
- **MCP統合**: Model Context Protocol（[rmcp](https://crates.io/crates/rmcp)）によるツール統合
- **ログ・監視**: 構造化ログ（JSON、stderr 出力）とメトリクス収集

## 必要なもの

- Rust 1.89 以降（edition 2024）

## ビルド・テスト

```bash
cargo build --release
cargo test
```

## 使用方法

### 基本的な使用例

```rust
use serde_json::json;
use subagent::{create_default_orchestrator, Task};

#[tokio::main]
async fn main() -> subagent::Result<()> {
    let orchestrator = create_default_orchestrator();

    let result = orchestrator
        .execute(
            Task::new("task-001", "research", json!({ "query": "最新のCI/CDトレンド", "depth": "detailed" })),
            None, // パターン未指定なら自動判定
        )
        .await?;

    println!("{}", serde_json::to_string_pretty(&result).unwrap());
    Ok(())
}
```

`execute` には `Task`・`Vec<Task>`・文字列（JSON または自由記述）を渡せます。

### 並列実行

```rust
use subagent::ExecutionPattern;

let tasks = vec![
    Task::new("task-1", "research", json!({ "query": "Rust best practices" })),
    Task::new("task-2", "codegen", json!({ "specification": "Create a utility function" })),
];
let result = orchestrator.execute(tasks, Some(ExecutionPattern::Parallel)).await?;
```

1つのタスクが失敗しても他のタスクは最後まで実行され、その後に最初のエラーが返ります。

### 逐次実行

```rust
use subagent::TaskContext;

let mut codegen = Task::new("task-2", "codegen", json!({ "specification": "Implement API client" }));
codegen.context = Some(TaskContext { parent_task: Some("task-1".into()), ..Default::default() });

let tasks = vec![
    Task::new("task-1", "research", json!({ "query": "API design patterns" })),
    codegen,
];
let result = orchestrator.execute(tasks, Some(ExecutionPattern::Sequential)).await?;
```

依存タスクの結果は `context.previous_results` に渡されます。

### イベントの購読

```rust
let mut events = orchestrator.subscribe(); // SubagentEvent::Progress / StatusChanged
```

### 出力先の指定

CodeGen / Document エージェントは既定でカレントディレクトリに出力します。出力先を変える場合:

```rust
use std::sync::Arc;
use subagent::{CodeGenAgent, Orchestrator};

let mut o = Orchestrator::new();
o.register_subagent(Arc::new(CodeGenAgent::with_options(
    CodeGenAgent::default_config(),
    Some("./out".into()),
)));
```

### サンプル

```bash
cargo run --example basic_usage
```

## MCPサーバー

各SubagentはMCPサーバー（stdio）として起動できます：

```bash
cargo build --release
./target/release/research-server
./target/release/codegen-server
./target/release/review-server
./target/release/document-server
```

クライアント設定例は [mcp-config.example.json](mcp-config.example.json) を参照してください。`command` の相対パスはクライアントの作業ディレクトリ基準で解決されるので、実際には絶対パスに書き換えてください。
各サーバーはエージェントのツールに加えて、タスクを丸ごと実行する `execute` ツールを公開します。

> **注意**: CodeGen の `bash_tool` / `file_create` / `str_replace` と Review の `bash_tool` は、任意のコマンド実行・任意パスへの書き込みができます。信頼できるクライアントからのみ使ってください。

### Review エージェントの lint

`source_code`（文字列）は必須で、`/` で始まるか絶対パスならファイルとして読み込みます。`lint_command` パラメータを指定したときだけ lint を実行します（例: `{"source_code": "...", "lint_command": "cargo clippy"}`）。出力は形式から言語を推定して解釈します。ESLint（stylish）・tsc・flake8 / ruff の行は `file` / `line` / `column` / `rule` / `tool` / `language` 付きの指摘になり、どれにも当たらない場合は `error` / `warning` を含む行が指摘になります（`tool: "generic"`）。stdout と stderr の両方を解釈します。ruff は既定の出力形式では行に当たらないので `ruff check --output-format concise` を指定してください。

### Document Agent の Notion 連携

Document Agent は次のツールを提供します。

- `notion_page_create`: 親ページ配下に新規作成 (`parent_page_id`, `title`, `content`)
- `notion_page_update`: 既存ページのタイトル変更・本文追記 (`page_id`, `title`, `content`)

利用には環境変数 `NOTION_TOKEN` に Notion integration のトークンを設定します。未設定の場合ツールはエラーを返します。対象ページは integration に共有してください。

```bash
NOTION_TOKEN=... ./target/release/document-server
```

HTTPS は rustls で、証明書の検証には OS の証明書ストア（rustls-platform-verifier）を使います。コンテナなどで動かす場合は CA 証明書（Debian / Ubuntu なら `ca-certificates`）を入れておいてください。タイムアウトは接続 10 秒・リクエスト全体 60 秒です。本文の追記がページ作成後に失敗した場合、エラーに作成済みの page_id を含めます（再実行で重複ページを作らないように `notion_page_update` で続きを追記してください）。

## ログ

ログは JSON で **stderr** に出力します（stdout は MCP の JSON-RPC 専用）。
`SUBAGENT_LOG_LEVEL` 環境変数で `debug` / `info`（既定） / `warn` / `error` / `off` を切り替えられます。

## プロジェクト構造

```
src/
├── lib.rs           # 公開 API と create_default_orchestrator
├── agents/          # Subagent 実装（research / codegen / review / document）
├── core/            # Subagent トレイト・共通状態・ツールレジストリ
├── orchestrator/    # Orchestrator・Task Parser・Planner・Result Aggregator
├── protocol.rs      # A2A プロトコル
├── tools/           # 組み込みツール（lint_parsers: lint 出力パーサ、notion: Notion API）
├── mcp.rs           # MCP サーバー
├── types.rs         # 型定義
├── error.rs         # エラー型
├── utils/           # ロガー・リトライ・エラーハンドラ
└── bin/             # MCP サーバーのエントリーポイント
tests/
├── orchestrator.rs  # Orchestrator の結合テスト
├── mcp_golden.rs    # MCP 出力を TS 版の記録（golden）と比較
└── golden/          # TS 版 MCP サーバーの出力記録
```

## ライセンス

MIT
