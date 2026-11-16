---
title: [E-4] selectSubagent を capability ベースに変更する
labels: enhancement
---

## Track / Worktree

- Track: **E Orchestrator**
- Branch: `cursor/orchestrator-capability-routing-acf1`
- Worktree: `../subagent-orchestrator-capability-routing`
- Owned paths: `src/orchestrator/orchestrator.ts`

## Goal

ハードコードされた agent ID（`research-001` 等）依存をやめ、capabilities / action マッチでルーティングする。

## Dependencies

- Blocks: —
- Blocked by: —

## Acceptance criteria

- [ ] action → capability マッピングで選択
- [ ] 未登録 capability は明確なエラー
- [ ] 既存 example が動く
- [ ] `npm run build` 成功
