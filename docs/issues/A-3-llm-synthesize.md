---
title: "[A-3] Research: LLM ベースの synthesize / confidence"
labels: enhancement
---

## Track / Worktree

- Track: **A Research**
- Branch: `cursor/research-llm-synthesize-acf1`
- Worktree: `../subagent-research-llm-synthesize`
- Owned paths: `src/agents/research/`

## Goal

文字列連結の synthesize を LLM 要約に置き換え、confidence を妥当に算出する。

## Dependencies

- Blocks: —
- Blocked by: A-1

## Acceptance criteria

- [ ] 要約品質がソースに基づく
- [ ] confidence が 0–1 で意味を持つ
- [ ] `npm run build` 成功
