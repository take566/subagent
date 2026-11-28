# golden ファイル

`tests/mcp_golden.rs` が比較に使う、移植元 TypeScript 版 MCP サーバーの出力記録です。
記録元は TS 版の最終コミット `29941ec`（origin/master、PR #34〜#54 を含む）です。

## 記録し直す手順

リポジトリのルートで実行します（手順 1 はサブシェルでビルドするので、作業ディレクトリは変わりません）。

```bash
# 1. TS 版を一時 worktree に取り出してビルド
git worktree add --detach /tmp/ts-master 29941ec
(cd /tmp/ts-master && npm ci && npx tsc)

# 2. 空の作業ディレクトリで各サーバーを起動し、呼び出し結果を記録
mkdir /tmp/work
node tests/golden/scripts/capture.mjs /tmp/ts-master/dist/mcp /tmp/raw /tmp/work

# 3. 実行ごとに変わる値（作業ディレクトリ・所要時間・タイムスタンプ）を正規化して保存
node tests/golden/scripts/sanitize.cjs /tmp/raw tests/golden /tmp/work

# 4. 一時 worktree を片付ける
git worktree remove --force /tmp/ts-master
```

- `capture.mjs` は `NOTION_TOKEN` / `AGENT_ID` / `MAX_CONCURRENT` を外してサーバーを起動します。
- `sanitize.cjs` は、シェルの出力に依存する `bash_tool` の呼び出しを除外します（OS ごとに出力が変わるため）。
- review の `execute` は TS 版が `npm run lint` を固定で実行するため `issues` を置き換えています。テスト側で Rust 版の期待値（lint なし）に上書きして比較します。
