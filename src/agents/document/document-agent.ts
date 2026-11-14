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
    const { path: filePath, content } = params as { path: string; content: string | Buffer };
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
      tools: ['file_create'],
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

    const tools: Tool[] = [new FileCreateTool()];

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
  private async formatContent(content: string, format: string): Promise<string | Buffer> {
    switch (format.toLowerCase()) {
      case 'docx':
        return this.formatAsDocx(content);
      case 'pdf':
        return this.formatAsPdf(content);
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
   * DOCX形式に変換（見出し `#` と段落のみ対応）
   * 依存は実行時に遅延読み込みする（docx 出力を使わない場合に不要にするため）
   */
  private async formatAsDocx(content: string): Promise<Buffer> {
    const moduleName = 'docx';
    const { Document, Packer, Paragraph, HeadingLevel } = await import(moduleName);
    const levels = [
      HeadingLevel.HEADING_1,
      HeadingLevel.HEADING_2,
      HeadingLevel.HEADING_3,
      HeadingLevel.HEADING_4,
      HeadingLevel.HEADING_5,
      HeadingLevel.HEADING_6,
    ];
    const children = content.split(/\r?\n/).map((line) => {
      const m = /^(#{1,6})\s+(.*)$/.exec(line);
      return m
        ? new Paragraph({ text: m[2], heading: levels[m[1].length - 1] })
        : new Paragraph({ text: line });
    });
    const doc = new Document({ sections: [{ children }] });
    return Buffer.from(await Packer.toBuffer(doc));
  }

  /**
   * PDF形式に変換（見出しの `#` 記号を除いて本文を出力）
   * 注意: PDFKit 標準フォントは日本語等の非ラテン文字を描画できない
   */
  private async formatAsPdf(content: string): Promise<Buffer> {
    const moduleName = 'pdfkit';
    const PDFDocument = (await import(moduleName)).default;
    const doc = new PDFDocument();
    const chunks: Buffer[] = [];
    const done = new Promise<Buffer>((resolve, reject) => {
      doc.on('data', (c: Buffer) => chunks.push(c));
      doc.on('end', () => resolve(Buffer.concat(chunks)));
      doc.on('error', reject);
    });
    for (const line of content.split(/\r?\n/)) {
      const m = /^(#{1,6})\s+(.*)$/.exec(line);
      if (m) {
        doc.fontSize(20 - m[1].length * 2).text(m[2]);
      } else {
        doc.fontSize(11).text(line);
      }
    }
    doc.end();
    return done;
  }

  /**
   * ファイルパスを生成
   */
  private generateFilePath(format: string): string {
    const timestamp = new Date().toISOString().replace(/[:.]/g, '-');
    const f = format.toLowerCase();
    const extension =
      f === 'docx' ? 'docx' : f === 'pdf' ? 'pdf' : f === 'markdown' || f === 'md' ? 'md' : f === 'html' ? 'html' : 'txt';
    return `docs/generated-${timestamp}.${extension}`;
  }
}

