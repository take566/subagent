---
title: "[D-1] Document: DOCX / PDF 出力サポート"
labels: enhancement
---

## Track / Worktree

- Track: **D Document**
- Branch: `cursor/document-docx-pdf-acf1`
- Worktree: `../subagent-document-docx-pdf`
- Owned paths: `src/agents/document/`

## Goal

MD/HTML 以外に DOCX / PDF 出力を追加する（設計 §5.4）。

## Dependencies

- Blocks: —
- Blocked by: —

## Acceptance criteria

- [ ] format=docx / pdf でファイル生成
- [ ] 依存ライブラリを package.json に追加
- [ ] `npm run build` 成功
