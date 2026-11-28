# Changelog

## [1.1.0] - 2026-11-28

### 変更
- 実装を TypeScript から Rust に移植（tokio / serde / rmcp）。TypeScript 版（src/*.ts, package.json など）は削除
- MCP サーバーは `target/release/*-server` のバイナリになった（`mcp-config.example.json` を更新）

### 修正
- ログを stdout ではなく stderr に出力するようにした（TS 版は MCP の stdio を汚していた）
- 並列実行で1つが失敗しても、他のタスクを最後まで実行してから最初のエラーを返すようにした。キャンセル時もエージェントの状態を戻す
- 進捗イベントを、エージェントの「現在のタスク」ではなく実行中のタスク ID に紐づけた（並列実行時の取り違えを防ぐ）
- シェル実行（`bash_tool`）で stdin を null にした（MCP サーバーの入力を子プロセスが読まないように）
- sequential で結果が空のとき、例外ではなくエラーを返すようにした

### 挙動の変更
- Review エージェントは `npm run lint` を固定で実行しなくなった。`lint_command` パラメータを指定したときだけ lint を実行する。`metrics.tools_called` は実際に呼んだツール数になった
- Review エージェントは Windows の絶対パス（`C:\...`）もファイルパスとして扱う
- CodeGen / Document エージェントに出力先ディレクトリ（`with_options` の `base_dir`）を追加した
- `str_replace` の `search` はリテラル文字列、`replace` も `$1` `$&` などを解釈しないそのままの文字列として扱う（TS #53 と同じ）。一致が 0 件ならファイルを書き戻さず `replaced: false` を返す。JS の置換規則を再現していた公開関数 `tools::js_replace_all` は削除した
- Review の lint 出力は ESLint stylish / tsc / flake8・ruff を言語別に解釈し、`file` `line` `column` `rule` `tool` `language` 付きの指摘にする（TS #41 と同じ）。TS との違い:
  - 行末の `\r` は落としてから解釈する（CRLF の出力も読める）
  - stdout に加えて stderr も解釈する（TS は stdout のみ）
- Document エージェントに Notion の `notion_page_create` / `notion_page_update` ツールを追加した（TS #45 と同じ。`NOTION_TOKEN` は呼び出し時に読む）。TS との違い:
  - 2000 文字ごとの分割で、サロゲートペア（絵文字など）を途中で切らない（TS は UTF-16 単位でそのまま切る）
  - 文字列でないパラメータは `InvalidParameter` エラーにする。`null` は未指定扱いで、`content: null` は空文字列として扱う（TS は `split` で例外）
  - 引数エラーのメッセージには `Invalid parameter: ` が前に付く（例: `Invalid parameter: page_id is required`）
  - ページ作成後の本文追記が失敗したら、エラーに作成済みの page_id を含める
  - HTTP は接続 10 秒・全体 60 秒でタイムアウトする
- 各エージェントに `default_tools` / `with_tools` を追加した（ツールを差し替えて作れる）
- エラー情報の `stack` は出力しない（`details.name` にエラー種別を入れる）。ファイル操作のエラーにはパスを含める
- 必要な Rust は 1.89 以降

### 挙動の変更（master の TS 修正 #38 / #40 / #44 / #54 の移植）
- `A2AProtocol::serialize` は `Result<String, String>` を返す。不正なメッセージは `Invalid A2A message: ...` のエラー
- `A2AProtocol::deserialize` のエラー型は `serde_json::Error` から `String` になった（`Invalid A2A message: malformed JSON` など）。`validate_value(&Value)` / `is_message` を追加
- `A2AProtocol::validate_message` は TS #38 と同じ厳密な検証（必須 key の型、timestamp、type、response の status、metadata、type ごとの payload）をする。`Orchestrator::handle_a2a_message` もこれを通るので、以前は受け付けていたメッセージ（payload 不正、timestamp 不正など）がエラー応答になる
- timestamp は ISO 8601 系（RFC 3339、タイムゾーン無しの日時、日付のみ。日付と時刻の区切りは `T` か空白）だけを有効とし、うるう秒（`:60`）は無効。JS の `Date.parse` が実装依存で受けるその他の書式（`2024/01/01`、RFC 2822 など）は受け付けない
- `mcp::run_stdio` の引数は `(default_config, build, label)` になった（`AGENT_ID` を反映した config からエージェントを作るため）
- MCP サーバーは `AGENT_ID`（agent id の上書き）と `MAX_CONCURRENT`（tools/call の同時実行数の上限）を読む
- MCP の `execute` は引数を agent に渡す前に検証する（TS #54）。`action` が空でない文字列でない、`task_id` が空でない文字列でない（数値も不可）、`parameters` が object でない場合は `Error: Invalid arguments: ...` の isError を返す
- `Logger::child` / `LogFields` を追加し、Subagent のツール呼び出し・進捗のログに `task_id` / `correlation_id` / `status` / `duration` を載せる
- Orchestrator のタスク実行ログを TS #44 に揃えた。`Executing task: <id>`（`status: "started"`）、`Task finished: <id>`（結果の `status` と `duration`）、`Task failed: <id>`（ERROR、`status: "failed"`、`duration`、`data.error`）に、そのタスクの `task_id` / `correlation_id` を載せる（correlation_id の無いタスクには載らない）
- `RetryManager` は `max_attempts = 0` のとき関数を呼ばずに `Retry failed` を返す（以前は 1 回呼んでいた。TS と同じ）

### 既知の制限
- `validate_value` / `is_message` は TS と同じ範囲しか見ないので、検証は通っても Rust の構造体に合わず `deserialize` だけ失敗するメッセージがある（例: task_request の `status: "bogus"`、`metadata.priority: "bogus"`）
- ツール呼び出しのログの `task_id` / `correlation_id` はエージェントの `current_task` から取るため、同じエージェントで複数タスクが並行して動くと別タスクの値が載ることがある（TS 版も同じ。#69）

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

