---
title: [A-2] Research: drive_search ツール追加とフォールバック配線
labels: enhancement
---

## Track / Worktree

- Track: **A Research**
- Branch: `cursor/research-drive-search-acf1`
- Worktree: `../subagent-research-drive-search`
- Owned paths: `src/agents/research/`

## Goal

設計どおり drive_search を追加し、web_search 失敗時のフォールバックに接続する。

## Dependencies

- Blocks: —
- Blocked by: A-1, H-2

## Acceptance criteria

- [ ] drive_search ツール登録
- [ ] ErrorHandler / fallback 経路で利用
- [ ] `npm run build` 成功
