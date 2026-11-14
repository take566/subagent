/**
 * Document Agent実装
 */

import { Subagent } from '../../core/subagent.js';
import type { Task, TaskResult, SubagentConfig, Tool } from '../../types/index.js';
import { BaseTool } from '../../core/tool.js';
import { RetryManager } from '../../utils/retry.js';
import { ErrorHandler } from '../../utils/error-handler.js';
import * as fs from 'fs/promises';
import * as path from 'path';

/**
 * ファイル作成ツール（Document用）
 */
class FileCreateTool extends BaseTool {
  constructor() {
    super('file_create', 'ファイルを作成', {
      path: { type: 'string', required: true },
      content: { type: 'string', required: true },
    });
  }

  async invoke(params: Record<string, any>): Promise<any> {
    const { path: filePath, content } = params;
    const fullPath = path.resolve(filePath);
    const dir = path.dirname(fullPath);

    await fs.mkdir(dir, { recursive: true });
    await fs.writeFile(fullPath, content, 'utf-8');

    return {
      path: fullPath,
      created: true,
      size: content.length,
    };
  }
}

const NOTION_API = 'https://api.notion.com/v1';
const NOTION_VERSION = '2022-06-28';

/**
 * Notion API 呼び出し。トークンは環境変数 NOTION_TOKEN から呼び出し時に読む。
 */
async function notionRequest(method: string, endpoint: string, body: unknown): Promise<any> {
  const token = process.env.NOTION_TOKEN;
  if (!token) {
    throw new Error('NOTION_TOKEN environment variable is not set');
  }
  const res = await fetch(`${NOTION_API}${endpoint}`, {
    method,
    headers: {
      Authorization: `Bearer ${token}`,
      'Notion-Version': NOTION_VERSION,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(body),
  });
  if (!res.ok) {
    throw new Error(`Notion API error: ${res.status} ${res.statusText}`);
  }
  return res.json();
}

/** テキストを Notion の paragraph ブロック列に変換 (rich_text は 2000 文字制限) */
function toParagraphBlocks(content: string): any[] {
  const blocks: any[] = [];
  for (const line of content.split('\n')) {
    if (line.trim() === '') continue;
    for (let i = 0; i < line.length; i += 2000) {
      blocks.push({
        object: 'block',
        type: 'paragraph',
        paragraph: { rich_text: [{ type: 'text', text: { content: line.slice(i, i + 2000) } }] },
      });
    }
  }
  return blocks;
}

/**
 * Notionページ作成ツール
 */
export class NotionPageCreateTool extends BaseTool {
  constructor() {
    super('notion_page_create', 'Notionページを作成', {
      parent_page_id: { type: 'string', required: true },
      title: { type: 'string', required: true },
      content: { type: 'string', required: false },
    });
  }

  async invoke(params: Record<string, any>): Promise<any> {
    const { parent_page_id, title, content = '' } = params;
    if (!parent_page_id || !title) {
      throw new Error('parent_page_id and title are required');
    }
    // 作成時の children は 100 ブロックまで。残りは追記する
    const blocks = toParagraphBlocks(content);
    const page = await notionRequest('POST', '/pages', {
      parent: { page_id: parent_page_id },
      properties: { title: { title: [{ type: 'text', text: { content: title } }] } },
      children: blocks.slice(0, 100),
    });
    for (let i = 100; i < blocks.length; i += 100) {
      await notionRequest('PATCH', `/blocks/${encodeURIComponent(page.id)}/children`, {
        children: blocks.slice(i, i + 100),
      });
    }
    return { page_id: page.id, url: page.url, created: true };
  }
}

/**
 * Notionページ更新ツール（タイトル変更と本文ブロックの追記）
 */
export class NotionPageUpdateTool extends BaseTool {
  constructor() {
    super('notion_page_update', 'Notionページを更新（タイトル変更・本文追記）', {
      page_id: { type: 'string', required: true },
      title: { type: 'string', required: false },
      content: { type: 'string', required: false },
    });
  }

  async invoke(params: Record<string, any>): Promise<any> {
    const { page_id, title, content } = params;
    if (!page_id) {
      throw new Error('page_id is required');
    }
    if (!title && !content) {
      throw new Error('title or content is required');
    }
    const id = encodeURIComponent(page_id);
    if (title) {
      await notionRequest('PATCH', `/pages/${id}`, {
        properties: { title: { title: [{ type: 'text', text: { content: title } }] } },
      });
    }
    if (content) {
      const blocks = toParagraphBlocks(content);
      for (let i = 0; i < blocks.length; i += 100) {
        await notionRequest('PATCH', `/blocks/${id}/children`, {
          children: blocks.slice(i, i + 100),
        });
      }
    }
    return { page_id, updated: true };
  }
}

export class DocumentAgent extends Subagent {
  private retryManager: RetryManager;
  private errorHandler: ErrorHandler;

  constructor(config?: Partial<SubagentConfig>) {
    const defaultConfig: SubagentConfig = {
      id: config?.id || 'document-001',
      name: config?.name || 'DocumentAgent',
      version: config?.version || '1.0.0',
      role: {
        description: 'ドキュメント作成・編集',
        capabilities: ['document_creation', 'formatting', 'translation'],
        constraints: {
          max_document_size: 1000000,
        },
      },
      interface: {
        input: [
          { name: 'content', type: 'string', required: true },
          { name: 'format', type: 'string', required: false },
        ],
        output: [
          {
            name: 'result',
            type: 'object',
            required: true,
            schema: {
              document_path: 'string',
              format: 'string',
            },
          },
        ],
      },
      tools: ['file_create', 'notion_page_create', 'notion_page_update'],
      behavior: {
        retry_policy: {
          max_attempts: 2,
          backoff: 'fixed',
          initial_delay_ms: 500,
        },
        timeout: 30000,
        fallback: 'return_partial',
      },
      ...config,
    };

    const tools: Tool[] = [
      new FileCreateTool(),
      new NotionPageCreateTool(),
      new NotionPageUpdateTool(),
    ];

    super(defaultConfig, tools);

    // retryManagerとerrorHandlerは将来の拡張用に保持
    // eslint-disable-next-line @typescript-eslint/no-unused-vars
    this.retryManager = new RetryManager(
      defaultConfig.behavior.retry_policy!,
      this.logger
    );
    this.errorHandler = new ErrorHandler(
      {
        validation_error: {
          action: 'request_clarification',
          escalate_after: 1,
        },
      },
      this.logger
    );
  }

  async execute(task: Task): Promise<TaskResult> {
    const startTime = Date.now();
    this.updateStatus('running');
    this.state.currentTask = task;

    try {
      const { content, format = 'markdown' } = task.parameters;

      await this.reportProgress(0.2, 'ドキュメント作成を開始');

      // 1. フォーマットに応じてコンテンツを変換
      const formattedContent = await this.formatContent(content, format);
      await this.reportProgress(0.5, 'フォーマット変換完了');

      // 2. ファイルパスを決定
      const filePath = this.generateFilePath(format);
      await this.reportProgress(0.7, 'ファイルパス決定');

      // 3. ファイル作成
      const result = await this.callTool('file_create', {
        path: filePath,
        content: formattedContent,
      });
      await this.reportProgress(0.9, 'ファイル作成完了');

      const duration = Date.now() - startTime;
      const taskResult: TaskResult = {
        taskId: task.id,
        status: 'completed',
        result: {
          document_path: result.path,
          format,
        },
        confidence: 0.95,
        metrics: {
          duration_ms: duration,
          tools_called: 1,
        },
      };

      this.addToHistory(taskResult);
      this.updateStatus('completed');
      this.state.currentTask = null;

      return taskResult;
    } catch (error) {
      const duration = Date.now() - startTime;
      const errorResult: TaskResult = {
        taskId: task.id,
        status: 'failed',
        error: this.errorHandler.createErrorInfo(
          error instanceof Error ? error : new Error(String(error)),
          'DOCUMENT_ERROR'
        ),
        metrics: {
          duration_ms: duration,
        },
      };

      this.addToHistory(errorResult);
      this.updateStatus('failed');
      this.state.currentTask = null;

      throw error;
    }
  }

  /**
   * コンテンツをフォーマット
   */
  private async formatContent(content: string, format: string): Promise<string> {
    switch (format.toLowerCase()) {
      case 'markdown':
      case 'md':
        return this.formatAsMarkdown(content);
      case 'html':
        return this.formatAsHTML(content);
      case 'text':
      case 'txt':
        return content;
      default:
        return content;
    }
  }

  /**
   * Markdown形式に変換
   */
  private formatAsMarkdown(content: string): string {
    // 実際の実装では、より高度な変換を実行
    return content;
  }

  /**
   * HTML形式に変換
   */
  private formatAsHTML(content: string): string {
    // 実際の実装では、MarkdownからHTMLへの変換を実行
    return `<html><body><pre>${content}</pre></body></html>`;
  }

  /**
   * ファイルパスを生成
   */
  private generateFilePath(format: string): string {
    const timestamp = new Date().toISOString().replace(/[:.]/g, '-');
    const extension = format === 'markdown' ? 'md' : format === 'html' ? 'html' : 'txt';
    return `docs/generated-${timestamp}.${extension}`;
  }
}

