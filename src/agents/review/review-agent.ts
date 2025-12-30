/**
 * Review Agent実装
 */

import { Subagent } from '../../core/subagent.js';
import type { Task, TaskResult, SubagentConfig, Tool } from '../../types/index.js';
import { BaseTool } from '../../core/tool.js';
import { RetryManager } from '../../utils/retry.js';
import { ErrorHandler } from '../../utils/error-handler.js';
import * as fs from 'fs/promises';

/**
 * ファイル閲覧ツール
 */
class ViewTool extends BaseTool {
  constructor() {
    super('view', 'ファイルの内容を閲覧', {
      path: { type: 'string', required: true },
    });
  }

  async invoke(params: Record<string, any>): Promise<any> {
    const { path: filePath } = params;
    const content = await fs.readFile(filePath, 'utf-8');
    return {
      path: filePath,
      content,
      size: content.length,
      lines: content.split('\n').length,
    };
  }
}

/**
 * Lint実行ツール
 */
class LintTool extends BaseTool {
  constructor() {
    super('bash_tool', 'Lintを実行', {
      command: { type: 'string', required: true },
      cwd: { type: 'string', required: false },
    });
  }

  async invoke(params: Record<string, any>): Promise<any> {
    const { command, cwd } = params;
    const { exec } = await import('child_process');
    const { promisify } = await import('util');
    const execAsync = promisify(exec);

    try {
      const { stdout, stderr } = await execAsync(command, { cwd });
      return {
        success: true,
        stdout,
        stderr,
        issues: this.parseLintOutput(stdout),
      };
    } catch (error: any) {
      return {
        success: false,
        error: error.message,
        stdout: error.stdout,
        stderr: error.stderr,
        issues: this.parseLintOutput(error.stdout || ''),
      };
    }
  }

  private parseLintOutput(output: string): any[] {
    // 実際の実装では、lint出力をパース
    const lines = output.split('\n');
    return lines
      .filter((line) => line.includes('error') || line.includes('warning'))
      .map((line) => ({
        type: line.includes('error') ? 'error' : 'warning',
        message: line,
      }));
  }
}

export class ReviewAgent extends Subagent {
  private retryManager: RetryManager;
  private errorHandler: ErrorHandler;

  constructor(config?: Partial<SubagentConfig>) {
    const defaultConfig: SubagentConfig = {
      id: config?.id || 'review-001',
      name: config?.name || 'ReviewAgent',
      version: config?.version || '1.0.0',
      role: {
        description: 'コードレビュー・品質チェック',
        capabilities: ['code_analysis', 'quality_check', 'security_scan'],
        constraints: {
          max_file_size: 500000,
        },
      },
      interface: {
        input: [
          { name: 'source_code', type: 'string', required: true },
          { name: 'criteria', type: 'object', required: false },
        ],
        output: [
          {
            name: 'result',
            type: 'object',
            required: true,
            schema: {
              issues: 'array',
              suggestions: 'array',
              score: 'number',
            },
          },
        ],
      },
      tools: ['view', 'bash_tool'],
      behavior: {
        retry_policy: {
          max_attempts: 2,
          backoff: 'linear',
          initial_delay_ms: 500,
        },
        timeout: 60000,
        fallback: 'return_partial',
      },
      ...config,
    };

    const tools: Tool[] = [new ViewTool(), new LintTool()];

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
      const { source_code, criteria = {} } = task.parameters;

      await this.reportProgress(0.2, 'コードレビューを開始');

      // 1. コードを読み込み
      let codeContent: string;
      if (typeof source_code === 'string' && source_code.startsWith('/')) {
        // ファイルパスの場合
        const fileInfo = await this.callTool('view', { path: source_code });
        codeContent = fileInfo.content;
      } else {
        // 直接コードが渡された場合
        codeContent = source_code;
      }
      await this.reportProgress(0.4, 'コード読み込み完了');

      // 2. Lint実行（可能な場合）
      let lintIssues: any[] = [];
      try {
        const lintResult = await this.callTool('bash_tool', {
          command: 'npm run lint 2>&1 || true',
        });
        if (lintResult.issues) {
          lintIssues = lintResult.issues;
        }
      } catch (error) {
        this.logger.warn('Lint実行に失敗しました', { error });
      }
      await this.reportProgress(0.6, 'Lint実行完了');

      // 3. コード分析
      const analysis = await this.analyzeCode(codeContent, criteria);
      await this.reportProgress(0.8, 'コード分析完了');

      // 4. レビュー結果を統合
      const issues = [...lintIssues, ...analysis.issues];
      const score = this.calculateScore(issues, analysis);

      await this.reportProgress(0.9, 'レビュー完了');

      const duration = Date.now() - startTime;
      const result: TaskResult = {
        taskId: task.id,
        status: 'completed',
        result: {
          issues,
          suggestions: analysis.suggestions,
          score,
        },
        confidence: 0.9,
        metrics: {
          duration_ms: duration,
          tools_called: 2,
        },
      };

      this.addToHistory(result);
      this.updateStatus('completed');
      this.state.currentTask = null;

      return result;
    } catch (error) {
      const duration = Date.now() - startTime;
      const errorResult: TaskResult = {
        taskId: task.id,
        status: 'failed',
        error: this.errorHandler.createErrorInfo(
          error instanceof Error ? error : new Error(String(error)),
          'REVIEW_ERROR'
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
   * コードを分析
   */
  private async analyzeCode(
    code: string,
    _criteria: Record<string, any>
  ): Promise<{ issues: any[]; suggestions: string[] }> {
    const issues: any[] = [];
    const suggestions: string[] = [];

    // 基本的なチェック
    if (code.length > 10000) {
      issues.push({
        type: 'warning',
        message: 'ファイルサイズが大きすぎます。分割を検討してください。',
        severity: 'medium',
      });
    }

    // コメントのチェック
    const commentRatio = (code.match(/\/\/|\/\*|\*/g) || []).length / code.split('\n').length;
    if (commentRatio < 0.1) {
      suggestions.push('コメントを追加してコードの可読性を向上させてください。');
    }

    // 実際の実装では、より詳細な分析を実行

    return { issues, suggestions };
  }

  /**
   * スコアを計算
   */
  private calculateScore(issues: any[], analysis: { issues: any[]; suggestions: string[] }): number {
    const errorCount = issues.filter((i) => i.type === 'error').length;
    const warningCount = issues.filter((i) => i.type === 'warning').length;

    let score = 100;
    score -= errorCount * 10;
    score -= warningCount * 2;
    score -= analysis.suggestions.length * 1;

    return Math.max(0, Math.min(100, score));
  }
}

