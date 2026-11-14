---
title: [C-1] Review: criteria 対応の静的解析・セキュリティチェック強化
labels: enhancement
---

## Track / Worktree

- Track: **C Review**
- Branch: `cursor/review-static-analysis-acf1`
- Worktree: `../subagent-review-static-analysis`
- Owned paths: `src/agents/review/`

## Goal

サイズ/コメント行数ヒューリスティックを超え、渡された criteria に基づく解析を行う。

## Dependencies

- Blocks: —
- Blocked by: —

## Acceptance criteria

- [ ] criteria がスコア / findings に反映
- [ ] セキュリティ観点の基本チェック
- [ ] `npm run build` 成功
