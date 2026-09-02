---
title: "[I-1] MCP: AGENT_ID / MAX_CONCURRENT 環境変数を反映する"
labels: enhancement
---

## Track / Worktree

- Track: **I MCP**
- Branch: `cursor/mcp-env-vars-acf1`
- Worktree: `../subagent-mcp-env-vars`
- Owned paths: `src/mcp/`

## Goal

mcp-config 例にある `AGENT_ID` / `MAX_CONCURRENT` をサーバー起動時に実際に反映する。

## Dependencies

- Blocks: J-2
- Blocked by: —

## Acceptance criteria

- [ ] AGENT_ID が agent id に反映
- [ ] MAX_CONCURRENT で同時実行制限
- [ ] `npm run build` 成功
