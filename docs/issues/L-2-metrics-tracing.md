---
title: [L-2] Observability: メトリクス（duration histogram, success rate）とトレース
labels: enhancement
---

## Track / Worktree

- Track: **L Observability**
- Branch: `cursor/obs-metrics-tracing-acf1`
- Worktree: `../subagent-obs-metrics-tracing`
- Owned paths: `src/obs/`（新規）, logger 連携

## Goal

histogram / gauge と W3C tracing（sample_rate 設定可）を追加する。

## Dependencies

- Blocks: —
- Blocked by: L-1

## Acceptance criteria

- [ ] subagent_task_duration / success_rate
- [ ] トレース伝播（少なくとも correlation）
- [ ] `npm run build` 成功
