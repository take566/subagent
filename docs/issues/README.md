# Issue バックログ（並行開発）

設計の正本: [`../parallel-development-design.md`](../parallel-development-design.md)

このディレクトリの Markdown は **1 ファイル = 1 Issue**。  
GitHub への一括作成:

```bash
chmod +x scripts/create-github-issues.sh
./scripts/create-github-issues.sh
```

Linear 利用時は各ファイルの `title` / 本文を転記するか、Linear MCP 認証後にエージェントへ作成依頼。

## Wave0（先行・並行 2）

- [F-1](./F-1-zod-validation.md)
- [F-2](./F-2-a2a-schema.md)

## Wave1（Orchestrator / Resilience / Tests）

- [E-1](./E-1-conditional-eval.md) … [E-4](./E-4-capability-routing.md)
- [H-1](./H-1-timeout-abort.md)
- [K-1](./K-1-test-orchestrator.md), [K-2](./K-2-test-protocol-utils.md)

## Wave2（Agents 最大並列）

- [A-1](./A-1-web-api.md), [B-1](./B-1-llm-pipeline.md), [C-1](./C-1-static-analysis.md), [C-2](./C-2-lint-parsers.md), [D-1](./D-1-docx-pdf.md), [D-2](./D-2-notion-tools.md)
- [E-5](./E-5-timeout-partial.md), [H-2](./H-2-error-handler-integrate.md)

## Wave3+

- [A-2](./A-2-drive-search.md), [A-3](./A-3-llm-synthesize.md), [B-2](./B-2-refactor-workflow.md), [H-3](./H-3-agent-retry.md), [K-3](./K-3-test-agents.md)

## Wave4–5

- [G-1](./G-1-shared-tools.md), [I-1](./I-1-mcp-env.md), [I-2](./I-2-mcp-input-schema.md), [J-1](./J-1-a2a-auth.md), [L-1](./L-1-structured-logging.md)
- [J-2](./J-2-sanitize-ratelimit.md), [L-2](./L-2-metrics-tracing.md), [K-4](./K-4-test-mcp.md)
