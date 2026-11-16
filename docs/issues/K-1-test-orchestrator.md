---
title: [K-1] test: orchestrator / planner / aggregator のユニットテスト
labels: enhancement
---

## Track / Worktree

- Track: **K Tests**
- Branch: `cursor/test-orchestrator-acf1`
- Worktree: `../subagent-test-orchestrator`
- Owned paths: `tests/orchestrator/` または `src/orchestrator/*.test.ts`

## Goal

TaskParser / Planner / ResultAggregator / Orchestrator のユニットテストを追加する。

## Dependencies

- Blocks: —
- Blocked by: E の主要変更が安定していること（推奨）

## Acceptance criteria

- [ ] topo sort / cycle / 各 pattern のケース
- [ ] `npm test` 成功
