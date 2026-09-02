---
title: "[G-1] FileCreate / Bash / View を共通モジュールに抽出する"
labels: enhancement
---

## Track / Worktree

- Track: **G Shared tools**
- Branch: `cursor/tools-shared-extract-acf1`
- Worktree: `../subagent-tools-shared-extract`
- Owned paths: `src/tools/`（新規）, 各 agent の import 差し替え

## Goal

エージェント間で重複している FileCreate / Bash / View を `src/tools/` に抽出する。

## Dependencies

- Blocks: —
- Blocked by: **A–D の主要 PR マージ後**（衝突回避）

## Acceptance criteria

- [ ] 共通モジュール化
- [ ] 各 agent が共通実装を参照
- [ ] 挙動回帰なし
- [ ] `npm run build` 成功

## Parallel note

Wave4。A–D 進行中は開始しない。
