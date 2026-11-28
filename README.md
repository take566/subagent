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

### CI

`.github/workflows/ci.yml` が PR・master への push・毎週月曜の定期実行・手動実行（workflow_dispatch）で次を検査します。

| ジョブ | 内容 |
| --- | --- |
| `rust (ubuntu-latest)` / `rust (windows-latest)` | `cargo fmt --check`、`cargo clippy --all-targets --locked -- -D warnings`、`cargo test --locked`、`cargo doc --no-deps --locked`（ubuntu のみ、警告はエラー） |
| `msrv (1.89)` | Cargo.toml の `rust-version` で `cargo check --all-targets --locked` |
| `deny` | [cargo-deny](https://github.com/EmbarkStudios/cargo-deny) で依存の脆弱性勧告・禁止事項（バージョン指定 `*` など）・ライセンス・取得元を検査（設定は `deny.toml`） |

手元で同じ検査をするには、CI と同じ版を `cargo install cargo-deny@0.20.2 --locked` で入れて `cargo deny --locked check` を実行します。
RustSec に新しい勧告が出ると PR でも `deny` が失敗します。すぐに対応できない勧告は、理由を書いて `deny.toml` の `ignore` に追加してください。
依存と GitHub Actions の更新は Dependabot（`.github/dependabot.yml`）が週に一度 PR を作ります。
依存の更新で必要な Rust の版が上がると `msrv` ジョブが失敗します。その場合は Cargo.toml の `rust-version` と `msrv` ジョブの toolchain を一緒に上げてください。
GitHub はリポジトリに 60 日間動きがないと定期実行を止めるので、その場合は Actions の画面から再度有効にしてください。

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

CodeGen / Document エージェントは既定で環境変数 `SUBAGENT_ROOTS` の workspace に出力します（未設定ならファイルを書くツールはエラーを返します。カレントディレクトリには書きません）。出力先をコードで指定する場合（無ければ作ります）:

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
`execute` の引数は `action`（必須、空でない文字列）、`task_id`（任意、空でない文字列）、`parameters`（任意、object）です。型が違うと `Error: Invalid arguments: ...` を返します。

| 環境変数 | 意味 |
|---|---|
| `AGENT_ID` | エージェントの id を上書きする（前後の空白は除く。空なら既定の id） |
| `MAX_CONCURRENT` | tools/call の同時実行数の上限。正の整数のときだけ有効で、未設定・不正値は無制限 |
| `SUBAGENT_ROOTS` | **必須**（ファイル・コマンドのツールを使う場合）。workspace の root の絶対パス。複数なら OS の path 区切り（Windows は `;`、それ以外は `:`）で並べ、相対パスは最初の root 基準。未設定ならツールは拒否する |
| `SUBAGENT_MAX_FILE_BYTES` | 1 ファイルの上限（既定 1048576 byte）。超えるファイルは読まず、書かない |
| `SUBAGENT_MAX_FILES` | 1 タスクで書き込むファイル数の上限（既定 200）。エージェントのタスク（`execute`）と direct tool の呼び出しごとに数え直す |
| `SUBAGENT_CODEGEN_ALLOWED_COMMANDS` | `bash_tool`（CodeGen / Review の lint）で実行してよい program 名（`,` か空白区切り、例 `cargo,npm`）。未設定ならコマンドは実行しない |
| `SUBAGENT_COMMAND_TIMEOUT_SECS` | コマンドの timeout（既定 120 秒）。超えたら kill する |

### sandbox（ファイルとコマンドの制限）

ファイルを扱うツール（CodeGen の `file_create` / `str_replace`、Review の `view`）は `SUBAGENT_ROOTS` の中だけを扱います。

- path は root からの相対パスだけ。絶対パス、`..`、`C:` などのドライブ指定、`\\?\` / UNC（`\\server\share`）、`:`（NTFS の代替データストリーム）、Windows の予約名（`CON` / `NUL` / `COM1` など。拡張子付きも）、末尾のドット・空白は拒否します（OS に関係なく）
- symlink / junction で root の外を指していれば拒否します。open した後にも handle の実パスを確かめるので、確認と open の間に差し替えられても root の外の中身は読まず、書きもしません
- 通常ファイル以外（FIFO・デバイス・ディレクトリ）は開きません。最後の component は link を辿らずに開きます（Unix は `O_NOFOLLOW | O_NONBLOCK`、Windows は `FILE_FLAG_OPEN_REPARSE_POINT`）
- 無いディレクトリは 1 段ずつ作り、作るたびに root の中か確かめます
- ハードリンク（リンク数 2 以上）のファイルは読みも書きもしません
- エラーメッセージには渡された相対パスだけを載せます（root の外の実パスは載せません）

既知の限界:

- 競合時（確認と作成の間に親ディレクトリを link に差し替えられた場合）、root の外に**空の**ディレクトリやファイルができることがあります。中身は書かず、既存のファイルを空にもしません
- 書き込み件数（`SUBAGENT_MAX_FILES`）はタスクごとです。同じエージェントで並行して動くタスクは、互いの数えをリセットしえます（上限は目安です）
- 作成・変更したファイルの記録（`Workspace::changes`）は、同じファイルを別名（8.3 形式の短い名前・大文字小文字違い）で書くと別々に載ります
- ファイル操作どうしは 1 つずつ実行しますが、コマンドの実行とは直列化しません（コマンドの実行中に同じファイルを読み書きできます）

`bash_tool` は shell を使いません（`cmd /c` / `sh -c` なし）。`command` は「program と引数」で、`"..."` / `'...'` で空白を含む引数を書けます。引用符の外の `|` `&` `;` `<` `>` `(` `)` `$` `` ` `` はエラーです。program は `SUBAGENT_CODEGEN_ALLOWED_COMMANDS` にある名前（path ではなく名前）だけで、PATH から探します。cwd は workspace（`cwd` 引数は root からの相対パス。`.` か空なら root）、stdin は null、出力は stdout / stderr それぞれ 3 万文字で切ります。

子プロセスの環境変数は `PATH` / `HOME` / `USERPROFILE` / `TEMP` / `TMP` / `TMPDIR` / `SystemRoot` と、Windows の tool chain が使う `APPDATA` / `LOCALAPPDATA` / `ProgramData` / `SystemDrive` / `PATHEXT` / `ComSpec` だけです。`ANTHROPIC_API_KEY` / `SUBAGENT_ANTHROPIC_API_KEY` / `NOTION_TOKEN` などは渡しません。`PATH` からは、相対の entry と workspace の中を指す entry を除きます（workspace に置いた同名の program を拾わないように）。

Windows では allowlist にも program にも `.exe` 以外の拡張子（`.bat` / `.cmd` など）は使えません（`cmd.exe` を通って実行されるため）。`npm` のような `.cmd` だけの program は実行できません。

> **警告: allowlist は sandbox ではありません。** `cargo` は `build.rs` を、`npm` は package の script を実行するので、許可した program を通じて任意のコードが動き、workspace の外も読み書きできます。timeout で kill するのは直接の子プロセスだけで、孫プロセスは残ることがあります。`cargo` / `npm` / `python` などを許可するのは、コンテナや WSL など、壊れても困らない隔離された環境の中だけにしてください。ホストで動かす場合は `SUBAGENT_CODEGEN_ALLOWED_COMMANDS` を設定しないでください。

### Review エージェントの lint

コードは `source_code`（文字列）か `source_path`（`SUBAGENT_ROOTS` 内の相対パス。`view` ツールで読みます）で渡します。1 行で `/` 始まりか絶対パスの `source_code` はファイルを読まずにエラーにします（workspace の外のファイルは読みません）。`lint_command` パラメータを指定したときだけ lint を実行します（例: `{"source_code": "...", "lint_command": "cargo clippy"}`）。lint も `bash_tool` と同じく shell なしで、program が `SUBAGENT_CODEGEN_ALLOWED_COMMANDS` に無ければ実行せずに（warn ログを出して）lint 無しで続けます。`cwd` パラメータは workspace 内の相対パスです。出力は形式から言語を推定して解釈します。ESLint（stylish）・tsc・flake8 / ruff の行は `file` / `line` / `column` / `rule` / `tool` / `language` 付きの指摘になり、どれにも当たらない場合は `error` / `warning` を含む行が指摘になります（`tool: "generic"`）。stdout と stderr の両方を解釈します。ruff は既定の出力形式では行に当たらないので `ruff check --output-format concise` を指定してください。

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
├── sandbox/         # Workspace（SUBAGENT_ROOTS）と run_command（allowlist）
├── tools/           # 組み込みツール（lint_parsers: lint 出力パーサ、notion: Notion API）
├── mcp.rs           # MCP サーバー
├── types.rs         # 型定義
├── error.rs         # エラー型
├── utils/           # ロガー・リトライ・エラーハンドラ
└── bin/             # MCP サーバーのエントリーポイント
tests/
├── orchestrator.rs  # Orchestrator の結合テスト
├── mcp_golden.rs    # MCP 出力を TS 版の記録（golden）と比較。sandbox の MCP 経由の確認も
├── sandbox.rs       # path の拒否・link での脱出・TOCTOU・上限・コマンドの制限
└── golden/          # TS 版 MCP サーバーの出力記録
```

## ライセンス

MIT
