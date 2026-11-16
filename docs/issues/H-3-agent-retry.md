---
title: [H-3] CodeGen/Review/Document でも RetryManager を実際に使う
labels: enhancement
---

## Track / Worktree

- Track: **H Resilience**
- Branch: `cursor/resilience-agent-retry-acf1`
- Worktree: `../subagent-resilience-agent-retry`
- Owned paths: `src/agents/codegen/`, `src/agents/review/`, `src/agents/document/`（retry 呼び出しのみ。ロジック変更は最小）

## Goal

Research 以外のエージェントでも RetryManager を利用する。

## Dependencies

- Blocks: —
- Blocked by: H-2

## Acceptance criteria

- [ ] 3 エージェントで一時失敗がリトライされる
- [ ] 所有エージェント以外を変更しない
- [ ] `npm run build` 成功

## Parallel note

A–D の大規模変更と同時進行する場合は PR コンフリクトに注意。Wave3 推奨。
