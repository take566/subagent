---
title: [B-2] CodeGen: リファクタ / str_replace ワークフローの実装
labels: enhancement
---

## Track / Worktree

- Track: **B CodeGen**
- Branch: `cursor/codegen-refactor-workflow-acf1`
- Worktree: `../subagent-codegen-refactor-workflow`
- Owned paths: `src/agents/codegen/`

## Goal

既存ファイルのリファクタ・部分置換ワークフローを実装する。

## Dependencies

- Blocks: —
- Blocked by: B-1

## Acceptance criteria

- [ ] view → 計画 → str_replace の一連フロー
- [ ] 失敗時のロールバック or 明確なエラー
- [ ] `npm run build` 成功
