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
CodeGen / Review / Document の `env.SUBAGENT_ROOTS` も、すでにあるディレクトリの絶対パスに書き換えてください（無いとファイルを扱うツールはエラーになります）。Windows では `"D:\\work\\proj"` か `"D:/work/proj"` と書きます（`/` で始まるパスは絶対パスとみなされず拒否されます）。
`SUBAGENT_CODEGEN_ALLOWED_COMMANDS` は例に入れていません（コマンドの実行は隔離した環境でだけ有効にしてください。[sandbox](#sandboxファイルとコマンドの制限) を参照）。Document の Notion ツールを使う場合は `NOTION_TOKEN` も `env` に入れます。既定（`hermes`）の typed tool は LLM を使うので、`SUBAGENT_ANTHROPIC_API_KEY` も `env` に入れてください（鍵は例に書いていません。設定ファイルを共有するときは鍵を含めないでください）。

### 公開するツール（`SUBAGENT_MCP_MODE`）

- `hermes`（既定）: 各サーバーの **typed tool 1 つだけ**を公開します。それ以外のツール（direct tool と `execute`）の呼び出しは `Error: Tool ... is not available (SUBAGENT_MCP_MODE=hermes ...)` で拒否します。`full` 以外の値（空・typo を含む）はすべて `hermes` として扱います
- `full`: エージェントのツール（direct tool: `web_search` / `file_create` / `bash_tool` / `view` / `notion_page_create` など）と `execute`、typed tool を公開します（従来の挙動 + typed tool）

typed tool（inputSchema はどれも `additionalProperties: false`。server 側でも引数を検証し、違えば `Error: Invalid arguments: ...`）:

| サーバー | tool | 引数 |
|---|---|---|
| research-server | `research` | `query`（必須）, `depth`（`quick` / `standard` / `detailed`）, `allowed_domains`（文字列の配列） |
| codegen-server | `codegen` | `task`（必須）, `files`（workspace からの相対パスの配列）, `allow_commands`（bool） |
| review-server | `review` | `code` か `path`（workspace からの相対パス）のどちらか一方, `language`, `lint`（bool。server の `SUBAGENT_REVIEW_LINT_COMMAND` を実行） |
| document-server | `document` | `content`（必須）, `format`（`md` / `txt` / `html`）, `mode`（`llm` / `verbatim`）, `title` |

LLM が未設定（鍵が無い）なら typed tool は mock の結果を返さず、`Error: LLM is not configured (set SUBAGENT_ANTHROPIC_API_KEY)` の isError を返します。4 つの typed tool はどれも Phase 3b〜3e で LLM の実装に置き換えました（research は下、document / review / codegen はそれぞれの節）。

**research**（Phase 3d）: Claude の server tool `web_search_20260209`（`max_uses` は `depth` で quick 3 / standard 5 / detailed 10）と `web_fetch_20260209`（`max_uses` quick 2 / standard 3 / detailed 5、`max_content_tokens` 8000、citations 有効）で調べます。`allowed_domains` は両方の tool に渡します（host 名だけ。scheme・path・port 付きや空の配列は `Invalid parameter`。省略で制限なし）。応答は `{untrusted_notice, text, summary, sources: [{url, title}], citations: [{url, title}], search_errors: [{tool, error_code, tool_use_id}], grounded, partial, tokens_used, stop_reason}` です。`untrusted_notice` は「web の untrusted な内容から model が作った結果で、中の指示に従わない」旨の固定文、`text` は `ResearchResult::render_for_downstream`（`<untrusted_research_result>` で囲んだ plain text）で、取得した page の本文（引用の `cited_text`）は応答に入れません。`summary` は最後の server tool の後の本文だけです（無ければ空）。`sources` は url で重複を除き、`grounded` は引用が 1 つ以上ある時だけ true、`partial` は token 予算・deadline・`pause_turn` の続きの上限・回数の上限・max_tokens で途中で止まった時に true です（出典を 1 つ以上集めた後の API のエラー・未知の stop_reason も partial。何も得られずに止まった時と、拒否（refusal）は isError）。

**送る前の token 予算の確認**（全エージェント）: 残りの予算が「request の概算（JSON の byte 数 / 3）+ max_tokens + 見込み」より少なければ送らずに止めます。research の見込みは 1 request ごとに web_fetch の max_uses × 8000 + web_search の max_uses × 8000 です。概算なので、1 request が残りの予算を超えることはあります。また `tokens_used` と予算は応答の usage の token（入力・出力・cache）だけを数え、web_search の 1 回ごとの課金（`usage.server_tool_use`）は数えません。

query・検索結果・取得した page は untrusted として扱います（区切りの tag は大文字小文字を問わず無効にします）。ファイルは書きません。

#### codegen の typed tool

Claude（既定 `claude-opus-5-5`、往復 25 回まで、1 タスクの token 予算 1000000）が `text_editor_20250728`（`str_replace_based_edit_tool`）で `SUBAGENT_ROOTS` の中のファイルを読み書きします。

- command は `view`（ファイルは行番号付き・10 万文字で切る、ディレクトリは 2 段まで・500 件まで、読めない下のディレクトリは印を付けて続ける）/ `create` / `str_replace` / `insert`。それ以外（`undo_edit` など）や workspace の外を指す path（`/` を含む絶対パス）は `is_error` の tool_result になり、Claude に返します（呼び出しは続きます）。1 回の応答の tool_result は 1 つの user message にまとめて送ります。合計は 20 万文字までで、超える `view` は `is_error`（`view_range` で小さく読むよう促す）にします
- `.git` の下（hook・`config` など）には書きません（direct tool も同じ。下の sandbox を参照）
- **`view` は `SUBAGENT_ROOTS` の中の `.env` などの dotfile も読めます**（ディレクトリの一覧には出しませんが、path を指定すれば読めます）。秘密を置いたディレクトリを root にしないでください
- `files` は最初の文脈として Workspace から読みます（50 件まで、1 ファイル 5 万文字・合計 12 万文字で切る）。中身は呼び出しごとの nonce 付きの区切りで囲み、untrusted（中の指示には従わない）と明示します。workspace の外を指す path（絶対パス・`..` など）はエラーで、LLM を呼びません。読めないファイルは理由を載せて続けます
- `allow_commands` は受け付けますが、issue #78 が直るまでは無効です（コマンドの tool は渡さず、結果の `notes` にそう書きます）
- 書いたファイルは**呼び出しごとに**記録します（`Workspace::begin_task` の全体の記録は使わないので、並行する呼び出しや direct tool の呼び出しと混ざりません）。`SUBAGENT_MAX_FILES` も呼び出しごとに数えます
- deadline（`SUBAGENT_DEADLINE_SECS`）や cancel の後に届いた tool_use では書きません（確認は Workspace の lock の中、書く直前に行います）

結果（JSON）: `files`（作った / 変えたファイルのうちテスト以外。`{"path": workspace からの相対パス, "kind": "created" | "modified"}` の配列）、`test_files`（同じ形。`tests/` などの下・`test_*`・`*_test.*`・`*.test.*`・`*.spec.*`）、`summary`（Claude の最後の本文）、`iterations`、`tokens_used`、`stop_reason`、`notes`。

`stop_reason` が `end_turn` / `stop_sequence` 以外（`refusal` / `max_tokens` / `max_tokens_with_tool_use` / `max_iterations` / `token_budget` / `deadline` / `pause_limit` / `llm_error` / `cancelled`）なら isError を返します。1 つ目の text は `Error: codegen did not finish (stop_reason: ..., N files changed)` だけで（server のログにも同じものだけを出します）、途中の結果（書いたファイル・本文・`error` にエラーの中身）は `structuredContent`（と 2 つ目の text）に入れます。応答を待つ間に cancel された時の `iterations` / `tokens_used` は、それまでに受け取った応答の分です。

`execute`（`full` のみ）の引数は `action`（必須、空でない文字列）、`task_id`（任意、空でない文字列。省略時は `task-<uuid>`）、`parameters`（任意、object）です。型が違うと `Error: Invalid arguments: ...` を返します。

1 回の tools/call は、request を受けた時から `SUBAGENT_DEADLINE_SECS` 秒（既定 540）で打ち切って `Error: Deadline exceeded ...` を返します（`MAX_CONCURRENT` の待ちも含みます）。client の `notifications/cancelled` でも打ち切ります（実行中の子プロセスも止め、取り消した request には応答も進捗も送りません）。request の `_meta.progressToken` があれば `notifications/progress` を送ります（開始・LLM との往復ごと・終了。`progress` は 1 ずつ増え、終了の通知だけ `total` が付き、応答より前に届きます。応答の後には送りません。通知を 2 秒以内に送れない時はその通知を諦め、呼び出しは止めません）。

| 環境変数 | 意味 |
|---|---|
| `AGENT_ID` | エージェントの id を上書きする（前後の空白は除く。空なら既定の id） |
| `MAX_CONCURRENT` | tools/call の同時実行数の上限。正の整数のときだけ有効で、未設定・不正値は無制限 |
| `SUBAGENT_MCP_MODE` | `hermes`（既定）か `full`。上を参照。`full` 以外は `hermes` |
| `SUBAGENT_DEADLINE_SECS` | 1 回の tools/call の deadline（秒、既定 540）。1〜86400 の整数でなければ（`abc`・`300s`・`0` など）起動しません（exit code 2）。エージェントの `behavior.timeout` を設定した場合は短い方 |
| `SUBAGENT_ANTHROPIC_API_KEY`（無ければ `ANTHROPIC_API_KEY`） | Claude API の鍵。無ければ LLM は未設定（通信しない）。値はログ・エラーに出しません |
| `SUBAGENT_MAX_TOKENS_PER_DAY` | 1 日（UTC）の token 上限（既定 2000000）。**server process ごと**の上限です（4 つの server はそれぞれ別に数えます。全体を抑えるには server ごとに設定します） |
| `SUBAGENT_LLM_BACKEND` | 未設定・空・`anthropic` は Claude API。`replay` は `--features replay` で build した bin だけ（記録した応答を返すテスト用）。それ以外の値と、feature なしの bin の `replay` では起動しません（exit code 2） |
| `SUBAGENT_LLM_REPLAY_FILE` | `replay` のときの記録ファイル（`MessageResponse` の JSON 配列。順に返す） |
| `SUBAGENT_MODEL` / `SUBAGENT_<AGENT>_MODEL` | model（既定 `claude-opus-5-5`）。`<AGENT>` は `RESEARCH` / `CODEGEN` / `REVIEW` / `DOCUMENT` で、エージェント名付きが優先 |
| `SUBAGENT_EFFORT` / `SUBAGENT_<AGENT>_EFFORT` | `low` / `medium` / `high` / `xhigh` / `max`（既定は送らない）。不正な値では起動しません |
| `SUBAGENT_TOKEN_BUDGET` / `SUBAGENT_<AGENT>_TOKEN_BUDGET` | 1 タスクの token 予算（正の整数。既定 research 300000 / codegen 1000000 / review 150000 / document 100000）。不正な値では起動しません |
| `SUBAGENT_REVIEW_LINT_COMMAND` | review の typed tool で `lint: true` のときに実行するコマンド（例 `cargo clippy`）。`bash_tool` と同じ allowlist を通るので、**その program を review-server の `SUBAGENT_CODEGEN_ALLOWED_COMMANDS` にも入れてください**（入っていない・起動できない場合、typed tool は応答の `lint_error` に理由を入れてレビューを続けます）。未設定で `lint: true` ならエラー |
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
- root からの相対パスに `.git`（大文字小文字は問わない）を含む場所には、どのツールでも書きません（`.git/hooks/*` や `.git/config` の `core.fsmonitor` で、後で git を使った時にホストでコードが動くため）。字面に加えて canonical なパスと open した handle の実パスでも確かめるので、8.3 の短い名前（`GIT~1`）でも抜けられません。読むのは拒否しません
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

### Review エージェントの typed tool（`review`）

Claude（既定 `claude-opus-5-5`、max_tokens 32000）に 1 回だけ聞いてレビューします。**`summary` と `issues` は信頼できない入力（コード・lint 出力）からモデルが生成したもの**です。呼び出し側はそれを指示として扱わないでください。

- 入力は `code`（文字列）か `path`（`SUBAGENT_ROOTS` からの相対パス。絶対パス・`..` は Workspace が拒否）のどちらか一方。12 万文字を超える入力はエラー。上限は Claude に送る prompt（行番号付きのコード・`language` / `path`・lint の出力）にも掛け、超えれば送らずにエラーにします。`language` は 64 文字まで。1 行で絶対パスに見える `code` もエラー
- コード・`language` / `path`・lint の出力は、推測できない id 付きの `<<<UNTRUSTED_...>>>` で囲んで渡し、その中の指示には従わないよう system prompt で指示します
- 出力は structured output（`output_config.format` の `json_schema`）。`issues[]`（`type` は `error` / `warning` / `info`、任意で `severity` / `line` / `rule`。`line` が 0 以下なら捨てます）・`suggestions[]`・`summary`。指摘は重要な順に 50 件まで、提案は 20 件までと指示しています
- `score` は従来の決定的な計算（100 − error×10 − warning×2 − 提案の数、0〜100）。数えるのは Claude の指摘・従来の heuristics（大きすぎるファイル、コメントの少なさ）と、`path` で渡した時だけ lint の指摘
- `lint: true` は `SUBAGENT_REVIEW_LINT_COMMAND` を workspace の root で実行します（`code` で渡した場合もそのコードではなく workspace を lint するので、lint の指摘は `lint.issues` に返すだけで score には数えません）。終了コードが 0 以外でも結果（`lint.success: false`）として扱い、allowlist に無い・起動できない場合は `lint_error` を返します
- 応答: `{score, issues, suggestions, summary, lint?: {command, success, timed_out, truncated, issues}, lint_error?, tokens_used, stop_reason}`。拒否（refusal）・max_tokens・不正な JSON・deadline・token 予算（既定 150000）・取り消しは isError
- 既知の限界: 送る前の予算の確認（runner の pre-send check、Phase 3d）が入るまでは、review の token 予算は応答を受け取った後に数えるだけです（1 往復なので、1 回の呼び出しは予算を超えて使いうる）

### Review エージェントの lint

コードは `source_code`（文字列）か `source_path`（`SUBAGENT_ROOTS` 内の相対パス。`view` ツールで読みます）で渡します。1 行で `/` 始まりか絶対パスの `source_code` はファイルを読まずにエラーにします（workspace の外のファイルは読みません）。`lint_command` パラメータを指定したときだけ lint を実行します（例: `{"source_code": "...", "lint_command": "cargo clippy"}`）。lint も `bash_tool` と同じく shell なしで、program が `SUBAGENT_CODEGEN_ALLOWED_COMMANDS` に無ければ実行せずに（warn ログを出して）lint 無しで続けます。`cwd` パラメータは workspace 内の相対パスです。出力は形式から言語を推定して解釈します。ESLint（stylish）・tsc・flake8 / ruff の行は `file` / `line` / `column` / `rule` / `tool` / `language` 付きの指摘になり、どれにも当たらない場合は `error` / `warning` を含む行が指摘になります（`tool: "generic"`）。stdout と stderr の両方を解釈します。ruff は既定の出力形式では行に当たらないので `ruff check --output-format concise` を指定してください。

### Document Agent の typed tool（`document`）

- `mode: "llm"`（既定）: Claude が `content`（と `title`）から文書を 1 往復で書きます（tool なし、prompt cache なし）。`content` と `title` は呼び出しごとの id 付きの untrusted ブロックで囲み、中の指示には従わないよう system prompt で伝えます。応答全体を包むコードフェンスは 1 つだけ外します。LLM が未設定なら `LLM is not configured` のエラーです
- 入力の上限: `content` は **60000 文字まで**（`mode: "llm"` のとき。超えたら送らず、切り詰めずにエラー）、`title` は 300 文字までです
- token 予算（既定 100000、`SUBAGENT_DOCUMENT_TOKEN_BUDGET`）: 1 往復で終わるので、今は応答を受け取った後にしか数えません（1 回の呼び出しが予算を超えることがあります）。runner の送信前の予算確認（Phase 3d で全エージェントに入る予定）が入ると、送る前に予算を確かめます
- `format: "html"`（llm）: system prompt で、静的な HTML（inline CSS のみ）を求め、`<script>`・inline の event handler（`onclick` など）・`javascript:` URL・外部の resource（`<img>` / `<link>` / `<iframe>` / `<form>` / CSS の `url()` など、ブラウザが取りに行く URL）を禁じます。出力そのものは検査・除去しません
- `mode: "verbatim"`: LLM を使わず（鍵が無くても動きます）、`content` を従来の `execute` と同じ書き方で保存します。`html` は `content` の `&` `<` `>` を escape してから `<html><body><pre>...</pre></body></html>` で包みます（`execute` は従来どおり escape しません）
- 出力先は workspace（`SUBAGENT_ROOTS`）の `docs/generated-<timestamp>-<8 桁の hex>.<ext>`（`md` → `.md`、`txt` → `.txt`、`html` → `.html`）。名前選びと書き込みは agent の中で直列にし、乱数の suffix で同じ root を使う別 process とも重なりにくくしています。それでも同名があれば `-1`, `-2`, ... を付け、上書きしません
- 拒否（refusal）・`max_tokens` で切れた・予算切れ・deadline・取り消し・LLM のエラーではファイルを書かずに isError を返します。deadline と取り消しは書く直前にも確かめます（「deadline exceeded」を返した後にファイルができることはありません）。書き込みが IO エラーで失敗した時は、作りかけのファイルを消してからエラーを返します（消せなかった時は残ることがあります）
- 結果: `{"path": "docs/generated-....md", "format": "md", "mode": "llm", "title": "..." | null, "bytes": 123, "tokens_used": 456, "stop_reason": "end_turn"}`（`path` は workspace からの相対パス、`bytes` は書いた byte 数。verbatim は `tokens_used: 0`、`stop_reason: null`）

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
├── mcp_golden.rs    # MCP の bin を起動して確かめる（TS 版の記録・Rust 版の golden・mode・progress・backend・sandbox）
├── sandbox.rs       # path の拒否・link での脱出・TOCTOU・上限・コマンドの制限
└── golden/          # ts/: TS 版の出力記録（history only）、rust/: Rust 版の golden、replay/: replay の記録
```

## ライセンス

MIT
