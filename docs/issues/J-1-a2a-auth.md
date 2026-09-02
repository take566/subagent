---
title: "[J-1] Security: A2A 認証（JWT/mTLS）と認可"
labels: enhancement
---

## Track / Worktree

- Track: **J Security**
- Branch: `cursor/security-a2a-auth-acf1`
- Worktree: `../subagent-security-a2a-auth`
- Owned paths: `src/security/`（新規）, `src/protocol/` のフック

## Goal

設計 §12 に沿い Agent 間通信の認証・ツール認可を実装する。

## Dependencies

- Blocks: —
- Blocked by: F-2

## Acceptance criteria

- [ ] JWT または mTLS の少なくとも一方
- [ ] 未認証メッセージ拒否
- [ ] `npm run build` 成功
