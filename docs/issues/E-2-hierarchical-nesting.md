---
title: "[E-2] Hierarchical を入れ子 Subagent 制御に対応する"
labels: enhancement
---

## Track / Worktree

- Track: **E Orchestrator**
- Branch: `cursor/orchestrator-hierarchical-nesting-acf1`
- Worktree: `../subagent-orchestrator-hierarchical-nesting`
- Owned paths: `src/orchestrator/`, 必要なら `src/core/subagent.ts` の spawn API（要コメント連携）

## Goal

親 Subagent が子を協調する本格 Hierarchical パターンを実装する（現状の parent-then-children 簡略実装を置き換え）。

## Dependencies

- Blocks: —
- Blocked by: core 側 spawn/delegate API の合意

## Acceptance criteria

- [ ] Coordinator Subagent から子タスク委譲が可能
- [ ] 結果が階層構造で集約される
- [ ] `npm run build` 成功
