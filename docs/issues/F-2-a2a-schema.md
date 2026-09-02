---
title: "[F-2] A2A メッセージの厳密な型ガードとペイロードスキーマ"
labels: enhancement
---

## Track / Worktree

- Track: **F Foundation**
- Branch: `cursor/foundation-a2a-schema-acf1`
- Worktree: `../subagent-foundation-a2a-schema`
- Owned paths: `src/protocol/`, `src/types/` の A2A 関連型のみ

## Goal

`A2AProtocol` の validate / serialize を厳密化し、不正メッセージを拒否するスキーマを導入する。

## Dependencies

- Blocks: I-2, J-1
- Blocked by: —

## Acceptance criteria

- [ ] task_request / task_result / progress 等の種別ごとのスキーマ
- [ ] 不正ペイロードで false / throw が明確
- [ ] ユニットテスト（K-2 と連携可）
- [ ] `npm run build` 成功

## Parallel note

Wave0。F-1 と並行可（衝突時は types の PR 順を調整）。
