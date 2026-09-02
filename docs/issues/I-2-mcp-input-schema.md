---
title: "[I-2] MCP: ツールスキーマを inputSchema として正しく公開する"
labels: enhancement
---

## Track / Worktree

- Track: **I MCP**
- Branch: `cursor/mcp-input-schema-acf1`
- Worktree: `../subagent-mcp-input-schema`
- Owned paths: `src/mcp/`

## Goal

ListTools で各ツールの JSON Schema を正確に公開する。

## Dependencies

- Blocks: —
- Blocked by: F-2

## Acceptance criteria

- [ ] inputSchema がツール定義と一致
- [ ] CallTool でスキーマ検証
- [ ] `npm run build` 成功
