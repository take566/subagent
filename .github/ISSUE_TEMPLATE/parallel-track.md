---
name: Parallel track issue
about: ワークツリー並行開発用 Issue
title: "[track-X] "
labels: enhancement
---

## Track / Worktree

- Track: `A|B|C|D|E|F|G|H|I|J|K|L`
- Branch: `cursor/<track>-<slug>-acf1`
- Worktree: `../subagent-<track>-<slug>`
- Owned paths:
  - `src/...`

## Goal

（1–3 文）

## Out of scope

- 他トラック所有パスの変更

## Dependencies

- Blocks:
- Blocked by:

## Acceptance criteria

- [ ] 所有パスのみ変更
- [ ] `npm run build` 成功
- [ ] テスト追加（該当時）`npm test` 成功
- [ ] 設計 doc `docs/parallel-development-design.md` の所有権ルールに準拠

## Notes

参照: `docs/parallel-development-design.md`
