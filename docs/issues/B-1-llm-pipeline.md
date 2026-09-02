---
title: "[B-1] CodeGen: LLM による本格コード生成パイプライン"
labels: enhancement
---

## Track / Worktree

- Track: **B CodeGen**
- Branch: `cursor/codegen-llm-pipeline-acf1`
- Worktree: `../subagent-codegen-llm-pipeline`
- Owned paths: `src/agents/codegen/`

## Goal

テンプレートスタブの generateCode を LLM パイプラインに置き換える。

## Dependencies

- Blocks: B-2
- Blocked by: —

## Acceptance criteria

- [ ] specification からコード生成
- [ ] 生成結果を file_create で書き出し可能
- [ ] `npm run build` 成功
