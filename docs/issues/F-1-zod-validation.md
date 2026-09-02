---
title: [F-1] SubagentConfig の入力バリデーションを zod で実装する
labels: enhancement
---

## Track / Worktree

- Track: **F Foundation**
- Branch: `cursor/foundation-zod-validation-acf1`
- Worktree: `../subagent-foundation-zod-validation`
- Owned paths: `src/types/`, 必要なら `src/core/subagent.ts` の呼び出し追加のみ

## Goal

設計スキーマ（`subagent-design-document.md` §3）に沿い、`SubagentConfig` / Task 入力を zod で実行時検証する。未使用の zod 依存を活用する。

## Dependencies

- Blocks: H, J, I-2
- Blocked by: —

## Acceptance criteria

- [ ] Input interface 必須項目の欠落で明確な ValidationError
- [ ] Orchestrator / Subagent.execute 入口で検証が走る
- [ ] 所有パス外の大規模変更なし
- [ ] `npm run build` 成功

## Parallel note

Wave0。F-2 と並行可（types vs protocol）。
