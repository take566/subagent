---
title: [E-3] ResultAggregator の親/子分離ロジックを修正する
labels: enhancement
---

## Track / Worktree

- Track: **E Orchestrator**
- Branch: `cursor/orchestrator-aggregator-parent-child-acf1`
- Worktree: `../subagent-orchestrator-aggregator-parent-child`
- Owned paths: `src/orchestrator/result-aggregator.ts`

## Goal

Hierarchical 集約で親フィルタが常に `true` になっている簡略実装を正しい親子分離に直す。

## Dependencies

- Blocks: —
- Blocked by: —

## Acceptance criteria

- [ ] 親結果と子結果が正しく分離・ネストされる
- [ ] ユニットテスト追加
- [ ] `npm run build` 成功

## Parallel note

Wave1。E-1/E-4 と並行可（ファイル衝突に注意。result-aggregator 専有）。
