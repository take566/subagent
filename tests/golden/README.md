# golden ファイル

`tests/mcp_golden.rs` が比較に使う記録です。

| ディレクトリ | 中身 | 更新 |
|---|---|---|
| `ts/` | 移植元 TypeScript 版 MCP サーバーの出力（history only） | 下の手順。普段は変えない |
| `rust/` | Rust 版の initialize、tools/list（`hermes` / `full`）、typed tool のエラーの形 | `GOLDEN_BLESS=1` |
| `replay/` | `SUBAGENT_LLM_BACKEND=replay` で返す応答の記録（`MessageResponse` の JSON 配列、server ごとに 1 つ・順に返す） | 手で書く |

## Rust 版（`rust/`）

`SUBAGENT_MCP_MODE`（既定 `hermes` と `full`）で各 server を起動し、initialize・tools/list と、typed tool に
不正な引数・未設定の LLM で呼んだ時の応答を記録します。意図して変えた時は記録し直してください。

```bash
GOLDEN_BLESS=1 cargo test --test mcp_golden rust_golden
git diff tests/golden/rust
```

`CI` が設定された環境で `GOLDEN_BLESS` を付けると test は失敗します（CI では記録し直さない）。
作業ディレクトリ（`<WORK>`）・uuid（`<UUID>`）・タイムスタンプ・所要時間は正規化して比較します。

子プロセスには API 鍵・`SUBAGENT_*` / `ANTHROPIC_*` の env を渡さず、`HTTP(S)_PROXY` を何も listen していない
`127.0.0.1:9` に向けます（どの test も外へ通信しません）。

## TS 版（`ts/`）

記録元は TS 版の最終コミット `29941ec`（origin/master、PR #34〜#54 を含む）です。Rust 版は `SUBAGENT_MCP_MODE=full`
で起動して比べます（typed tool は TS 版に無いので一覧の最後の 1 つを除いて比べます）。

意図した差分（test 側で期待値を上書きします）:

- review の `execute` は TS 版が `npm run lint` を固定で実行するため `issues` / `score` / `tools_called` を置き換える
- Notion ツールの引数エラーには `Invalid parameter: ` が付く（CHANGELOG 1.1.0）
- codegen の `bash_tool` の説明は shell を使わないことに合わせて変えた（TS 版は「Bashコマンドを実行」）

### 記録し直す手順

リポジトリのルートで実行します（手順 1 はサブシェルでビルドするので、作業ディレクトリは変わりません）。

```bash
# 1. TS 版を一時 worktree に取り出してビルド
git worktree add --detach /tmp/ts-master 29941ec
(cd /tmp/ts-master && npm ci && npx tsc)

# 2. 空の作業ディレクトリで各サーバーを起動し、呼び出し結果を記録
mkdir /tmp/work
node tests/golden/scripts/capture.mjs /tmp/ts-master/dist/mcp /tmp/raw /tmp/work

# 3. 実行ごとに変わる値（作業ディレクトリ・所要時間・タイムスタンプ）を正規化して保存
node tests/golden/scripts/sanitize.cjs /tmp/raw tests/golden/ts /tmp/work

# 4. 一時 worktree を片付ける
git worktree remove --force /tmp/ts-master
```

- `capture.mjs` は `NOTION_TOKEN` / `AGENT_ID` / `MAX_CONCURRENT` を外してサーバーを起動します。
- `sanitize.cjs` は、シェルの出力に依存する `bash_tool` の呼び出しを除外します（OS ごとに出力が変わるため）。
