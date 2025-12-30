/**
 * Research Agent実装
 */

import { Subagent } from '../../core/subagent.js';
import type { Task, TaskResult, SubagentConfig, Tool } from '../../types/index.js';
import { BaseTool } from '../../core/tool.js';
import { RetryManager } from '../../utils/retry.js';
import { ErrorHandler } from '../../utils/error-handler.js';

/**
 * Web検索ツール（モック実装）
 */
class WebSearchTool extends BaseTool {
  constructor() {
    super('web_search', 'Web検索を実行', {
      query: { type: 'string', required: true },
      max_results: { type: 'number', required: false },
    });
  }

  async invoke(params: Record<string, any>): Promise<any> {
    // 実際の実装では、web_search APIを呼び出す
    const { query } = params;
    return {
      results: [
        {
          title: `検索結果: ${query}`,
          url: 'https://example.com/result1',
          snippet: `${query}に関する情報です。`,
        },
      ],
      query,
      count: 1,
    };
  }
}

/**
 * Web取得ツール（モック実装）
 */
class WebFetchTool extends BaseTool {
  constructor() {
    super('web_fetch', 'Webページの内容を取得', {
      url: { type: 'string', required: true },
    });
  }

  async invoke(params: Record<string, any>): Promise<any> {
    const { url } = params;
    // 実際の実装では、HTTPリクエストを実行
    return {
      url,
      content: `ページの内容: ${url}`,
      title: 'ページタイトル',
    };
  }
}

/**
 * ドキュメント解析ツール（モック実装）
 */
class DocumentParserTool extends BaseTool {
  constructor() {
    super('document_parser', 'ドキュメントを解析', {
      content: { type: 'string', required: true },
      format: { type: 'string', required: false },
    });
  }

  async invoke(params: Record<string, any>): Promise<any> {
    // contentパラメータは将来の実装で使用
    return {
      parsed: true,
      sections: ['セクション1', 'セクション2'],
      summary: 'ドキュメントの要約',
    };
  }
}

export class ResearchAgent extends Subagent {
  private retryManager: RetryManager;
  private errorHandler: ErrorHandler;

  constructor(config?: Partial<SubagentConfig>) {
    const defaultConfig: SubagentConfig = {
      id: config?.id || 'research-001',
      name: config?.name || 'ResearchAgent',
      version: config?.version || '1.0.0',
      role: {
        description: '情報収集と要約を担当',
        capabilities: ['web_search', 'document_analysis', 'summarization'],
        constraints: {
          max_search_queries: 10,
          max_tokens_output: 4000,
        },
      },
      interface: {
        input: [
          { name: 'query', type: 'string', required: true },
          { name: 'context', type: 'object', required: false },
        ],
        output: [
          {
            name: 'result',
            type: 'object',
            required: true,
            schema: {
              summary: 'string',
              sources: 'array',
              confidence: 'number',
            },
          },
        ],
      },
      tools: ['web_search', 'web_fetch', 'document_parser'],
      behavior: {
        retry_policy: {
          max_attempts: 3,
          backoff: 'exponential',
          initial_delay_ms: 1000,
        },
        timeout: 60000,
        fallback: 'return_partial',
      },
      ...config,
    };

    const tools: Tool[] = [
      new WebSearchTool(),
      new WebFetchTool(),
      new DocumentParserTool(),
    ];

    super(defaultConfig, tools);

    this.retryManager = new RetryManager(
      defaultConfig.behavior.retry_policy!,
      this.logger
    );
    this.errorHandler = new ErrorHandler(
      {
        timeout: {
          action: 'retry_with_backoff',
          max_retries: 3,
          fallback: 'delegate_to_alternative',
        },
        tool_failure: {
          action: 'use_alternative_tool',
          alternatives: [
            { primary: 'web_search', backup: 'document_parser' },
          ],
        },
        partial_failure: {
          action: 'return_partial_with_flag',
          notify: true,
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
      const { query, depth = 'standard' } = task.parameters;

      await this.reportProgress(0.1, '検索を開始');

      // 1. 検索実行
      const searchResults = await this.retryManager.executeWithRetry(async () => {
        return await this.callTool('web_search', {
          query,
          max_results: depth === 'detailed' ? 10 : 5,
        });
      });

      await this.reportProgress(0.3, '検索完了');

      // 2. 詳細取得
      const topResults = searchResults.results.slice(0, 3);
      const details = await Promise.all(
        topResults.map((result: any) =>
          this.retryManager.executeWithRetry(async () => {
            return await this.callTool('web_fetch', { url: result.url });
          })
        )
      );

      await this.reportProgress(0.7, '詳細取得完了');

      // 3. 要約生成
      const summary = await this.synthesize(details, query);
      await this.reportProgress(0.9, '要約生成完了');

      const duration = Date.now() - startTime;
      const result: TaskResult = {
        taskId: task.id,
        status: 'completed',
        result: {
          summary,
          sources: searchResults.results,
          confidence: this.calculateConfidence(details),
        },
        confidence: this.calculateConfidence(details),
        metrics: {
          duration_ms: duration,
          tools_called: 1 + details.length,
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
          'RESEARCH_ERROR'
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
   * 情報を統合して要約を生成
   */
  private async synthesize(details: any[], query: string): Promise<string> {
    // 実際の実装では、LLMを使用して要約を生成
    const summaries = details.map((d) => d.content || d.summary || '').join('\n\n');
    return `「${query}」に関する調査結果:\n\n${summaries.substring(0, 1000)}...`;
  }

  /**
   * 信頼度を計算
   */
  private calculateConfidence(details: any[]): number {
    if (details.length === 0) return 0.0;
    if (details.length >= 3) return 0.9;
    if (details.length >= 2) return 0.7;
    return 0.5;
  }
}

