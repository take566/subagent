---
title: "[E-5] 実行タイムアウトと partial failure の集約"
labels: enhancement
---

## Track / Worktree

- Track: **E Orchestrator**
- Branch: `cursor/orchestrator-timeout-partial-acf1`
- Worktree: `../subagent-orchestrator-timeout-partial`
- Owned paths: `src/orchestrator/`

## Goal

タイムアウト・部分失敗時に Aggregator が `partial` フラグ付きで結果を返すようにする。

## Dependencies

- Blocks: —
- Blocked by: H-1

## Acceptance criteria

- [ ] 一部失敗でも他結果を保持
- [ ] metrics に failure count
- [ ] `npm run build` 成功
