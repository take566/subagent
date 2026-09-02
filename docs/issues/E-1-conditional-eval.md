---
title: [E-1] Conditional 条件評価を拡張する（式パーサ / 安全評価）
labels: enhancement
---

## Track / Worktree

- Track: **E Orchestrator**
- Branch: `cursor/orchestrator-conditional-eval-acf1`
- Worktree: `../subagent-orchestrator-conditional-eval`
- Owned paths: `src/orchestrator/planner.ts`, 必要なら専用ヘルパ

## Goal

現状の簡易文字列マッチ（`previous.success` 等）を、安全な条件式評価に置き換える。

## Dependencies

- Blocks: —
- Blocked by: F-1（任意）

## Acceptance criteria

- [ ] boolean / 比較 / 前タスク結果参照をサポート
- [ ] 任意コード実行は不可（サンドボックス or 許可リスト）
- [ ] 既存 sequential/parallel を壊さない
- [ ] `npm run build` 成功
