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
- MCP の `execute` に数値の `task_id` を渡すと、文字列化した ID になる（TS 版は数値のまま）
- 必要な Rust は 1.89 以降

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

