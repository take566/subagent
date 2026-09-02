---
title: "[L-1] Observability: task_id / correlation_id 付き構造化ログ"
labels: enhancement
---

## Track / Worktree

- Track: **L Observability**
- Branch: `cursor/obs-structured-logging-acf1`
- Worktree: `../subagent-obs-structured-logging`
- Owned paths: `src/utils/logger.ts`

## Goal

設計 §11 のフィールド（agent_id, task_id, correlation_id, duration, status）をログに載せる。

## Dependencies

- Blocks: L-2
- Blocked by: —

## Acceptance criteria

- [ ] JSON ログに必須フィールド
- [ ] Orchestrator / Subagent から伝播
- [ ] `npm run build` 成功
