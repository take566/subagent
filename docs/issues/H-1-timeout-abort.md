---
title: [H-1] behavior.timeout を AbortSignal で強制する
labels: enhancement
---

## Track / Worktree

- Track: **H Resilience**
- Branch: `cursor/resilience-timeout-abort-acf1`
- Worktree: `../subagent-resilience-timeout-abort`
- Owned paths: `src/core/subagent.ts`, `src/utils/`

## Goal

設定上の `behavior.timeout` を実際に強制し、超過時は AbortSignal で中断する。

## Dependencies

- Blocks: H-2, E-5
- Blocked by: —

## Acceptance criteria

- [ ] timeout 超過で failed + 理由が残る
- [ ] ツール呼び出しにも伝播可能な AbortSignal
- [ ] `npm run build` 成功
