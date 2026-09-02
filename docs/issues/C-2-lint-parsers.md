---
title: "[C-2] Review: Lint 出力パーサの言語別対応"
labels: enhancement
---

## Track / Worktree

- Track: **C Review**
- Branch: `cursor/review-lint-parsers-acf1`
- Worktree: `../subagent-review-lint-parsers`
- Owned paths: `src/agents/review/`

## Goal

bash 経由の lint 出力を言語別にパースし、構造化 findings にする。

## Dependencies

- Blocks: —
- Blocked by: —

## Acceptance criteria

- [ ] 少なくとも TS/ESLint と 1 他言語
- [ ] findings スキーマに正規化
- [ ] `npm run build` 成功

## Parallel note

C-1 と同ディレクトリのため、同時進行時はブランチを分割し早めにマージ。
