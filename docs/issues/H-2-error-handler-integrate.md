---
title: "[H-2] ErrorHandler を Subagent.execute に統合する"
labels: enhancement
---

## Track / Worktree

- Track: **H Resilience**
- Branch: `cursor/resilience-error-handler-integrate-acf1`
- Worktree: `../subagent-resilience-error-handler-integrate`
- Owned paths: `src/utils/error-handler.ts`, `src/core/subagent.ts`

## Goal

代替ツール・エスカレーション・partial return など ErrorHandler 戦略を execute パスに配線する。

## Dependencies

- Blocks: H-3, A-2
- Blocked by: H-1

## Acceptance criteria

- [ ] tool_failure で代替ツール試行
- [ ] escalate 条件で orchestrator/human 向けフラグ
- [ ] `npm run build` 成功
