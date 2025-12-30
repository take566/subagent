/**
 * MCP Server実装
 */

import { Server } from '@modelcontextprotocol/sdk/server/index.js';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import {
  CallToolRequestSchema,
  ListToolsRequestSchema,
} from '@modelcontextprotocol/sdk/types.js';
import { Subagent } from '../core/subagent.js';
import type { Task } from '../types/index.js';
import { Logger } from '../utils/logger.js';

export class MCPServer {
  private server: Server;
  private subagent: Subagent;
  private logger: Logger;

  constructor(subagent: Subagent) {
    this.subagent = subagent;
    this.logger = new Logger(`mcp-${subagent.getConfig().id}`);
    this.server = new Server(
      {
        name: subagent.getConfig().name,
        version: subagent.getConfig().version,
      },
      {
        capabilities: {
          tools: {},
        },
      }
    );

    this.setupHandlers();
  }

  /**
   * ハンドラーを設定
   */
  private setupHandlers(): void {
    // ツール一覧を取得
    this.server.setRequestHandler(ListToolsRequestSchema, async () => {
      const tools = this.subagent.getAvailableTools().map((toolName) => {
        const tool = this.subagent.getTool(toolName);
        return {
          name: toolName,
          description: tool?.description || '',
          inputSchema: {
            type: 'object',
            properties: tool?.parameters || {},
          },
        };
      });

      // executeツールを追加
      tools.push({
        name: 'execute',
        description: 'タスクを実行',
        inputSchema: {
          type: 'object',
          properties: {
            task_id: {
              type: 'string',
              description: 'タスクID',
            },
            action: {
              type: 'string',
              description: 'アクション名',
            },
            parameters: {
              type: 'object',
              description: 'パラメータ',
            },
          },
        },
      });

      return { tools };
    });

    // ツールを呼び出し
    this.server.setRequestHandler(CallToolRequestSchema, async (request) => {
      const { name, arguments: args } = request.params;

      if (!args) {
        return {
          content: [
            {
              type: 'text',
              text: 'Error: Missing arguments',
            },
          ],
          isError: true,
        };
      }

      try {
        if (name === 'execute') {
          // タスクを実行
          const task: Task = {
            id: (args.task_id as string) || `task-${Date.now()}`,
            action: args.action as string,
            parameters: (args.parameters as Record<string, any>) || {},
          };

          const result = await this.subagent.execute(task);
          return {
            content: [
              {
                type: 'text',
                text: JSON.stringify(result, null, 2),
              },
            ],
          };
        } else {
          // 直接ツールを呼び出し
          const result = await this.subagent.invokeTool(name, args as Record<string, any>);
          return {
            content: [
              {
                type: 'text',
                text: JSON.stringify(result, null, 2),
              },
            ],
          };
        }
      } catch (error) {
        this.logger.error(`Tool execution failed: ${name}`, { error });
        return {
          content: [
            {
              type: 'text',
              text: `Error: ${error instanceof Error ? error.message : String(error)}`,
            },
          ],
          isError: true,
        };
      }
    });
  }

  /**
   * サーバーを起動
   */
  async start(): Promise<void> {
    const transport = new StdioServerTransport();
    await this.server.connect(transport);
    this.logger.info('MCP Server started', {
      agent: this.subagent.getConfig().name,
    });
  }
}

