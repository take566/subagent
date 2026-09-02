---
title: "[A-1] Research: web_search / web_fetch の実 API 接続"
labels: enhancement
---

## Track / Worktree

- Track: **A Research**
- Branch: `cursor/research-web-api-acf1`
- Worktree: `../subagent-research-web-api`
- Owned paths: `src/agents/research/`

## Goal

モックの web_search / web_fetch を実 API（または設定可能なプロバイダ）に置き換える。

## Dependencies

- Blocks: A-2, A-3
- Blocked by: —

## Acceptance criteria

- [ ] 実ネットワーク呼び出し（失敗時は明確なエラー）
- [ ] API キーは環境変数
- [ ] `npm run build` 成功
