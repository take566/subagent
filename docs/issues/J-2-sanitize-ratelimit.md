---
title: "[J-2] Security: 出力サニタイズとレート制限"
labels: enhancement
---

## Track / Worktree

- Track: **J Security**
- Branch: `cursor/security-sanitize-ratelimit-acf1`
- Worktree: `../subagent-security-sanitize-ratelimit`
- Owned paths: `src/security/`, 必要なら `src/core/`

## Goal

出力の機密マスキングと Subagent 単位のレート制限を実装する。

## Dependencies

- Blocks: —
- Blocked by: I-1（並行度設定連携）

## Acceptance criteria

- [ ] 機密パターンのマスキング
- [ ] レート超過時の明確なエラー
- [ ] `npm run build` 成功
