/**
 * Code Generation Agent実装
 */

import { Subagent } from '../../core/subagent.js';
import type { Task, TaskResult, SubagentConfig, Tool } from '../../types/index.js';
import { BaseTool } from '../../core/tool.js';
import { RetryManager } from '../../utils/retry.js';
import { ErrorHandler } from '../../utils/error-handler.js';
import * as fs from 'fs/promises';
import * as path from 'path';

/**
 * ファイル作成ツール
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

    // ディレクトリが存在しない場合は作成
    await fs.mkdir(dir, { recursive: true });

    // ファイルを作成
    await fs.writeFile(fullPath, content, 'utf-8');

    return {
      path: fullPath,
      created: true,
      size: content.length,
    };
  }
}

/**
 * Bash実行ツール
 */
class BashTool extends BaseTool {
  constructor() {
    super('bash_tool', 'Bashコマンドを実行', {
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
      };
    } catch (error: any) {
      return {
        success: false,
        error: error.message,
        stdout: error.stdout,
        stderr: error.stderr,
      };
    }
  }
}

/**
 * 文字列置換ツール
 */
class StrReplaceTool extends BaseTool {
  constructor() {
    super('str_replace', '文字列を置換', {
      file: { type: 'string', required: true },
      search: { type: 'string', required: true },
      replace: { type: 'string', required: true },
    });
  }

  async invoke(params: Record<string, any>): Promise<any> {
    const { file, search, replace } = params;
    const content = await fs.readFile(file, 'utf-8');
    const newContent = content.replace(new RegExp(search, 'g'), replace);
    await fs.writeFile(file, newContent, 'utf-8');

    return {
      file,
      replaced: true,
      occurrences: (content.match(new RegExp(search, 'g')) || []).length,
    };
  }
}

export class CodeGenAgent extends Subagent {
  private retryManager: RetryManager;
  private errorHandler: ErrorHandler;

  constructor(config?: Partial<SubagentConfig>) {
    const defaultConfig: SubagentConfig = {
      id: config?.id || 'codegen-001',
      name: config?.name || 'CodeGenAgent',
      version: config?.version || '1.0.0',
      role: {
        description: 'コード生成・リファクタリング',
        capabilities: ['code_generation', 'refactoring', 'test_generation'],
        constraints: {
          max_file_size: 100000,
          max_files_per_task: 50,
        },
      },
      interface: {
        input: [
          { name: 'specification', type: 'string', required: true },
          { name: 'language', type: 'string', required: false },
          { name: 'framework', type: 'string', required: false },
        ],
        output: [
          {
            name: 'result',
            type: 'object',
            required: true,
            schema: {
              files: 'array',
              test_files: 'array',
            },
          },
        ],
      },
      tools: ['file_create', 'bash_tool', 'str_replace'],
      behavior: {
        retry_policy: {
          max_attempts: 3,
          backoff: 'exponential',
          initial_delay_ms: 1000,
        },
        timeout: 120000,
        fallback: 'return_partial',
      },
      ...config,
    };

    const tools: Tool[] = [
      new FileCreateTool(),
      new BashTool(),
      new StrReplaceTool(),
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
          escalate_after: 2,
        },
        tool_failure: {
          action: 'use_alternative_tool',
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
      const { specification, language = 'typescript', framework } = task.parameters;

      await this.reportProgress(0.2, 'コード生成を開始');

      // 1. コード生成（実際の実装ではLLMを使用）
      const generatedCode = await this.generateCode(specification, language, framework);
      await this.reportProgress(0.5, 'コード生成完了');

      // 2. ファイル作成
      const files = [];
      for (const [filePath, content] of Object.entries(generatedCode.files)) {
        const result = await this.callTool('file_create', {
          path: filePath,
          content: content as string,
        });
        files.push(result);
      }
      await this.reportProgress(0.7, 'ファイル作成完了');

      // 3. テストコード生成（オプション）
      const testFiles = [];
      if (generatedCode.tests) {
        for (const [filePath, content] of Object.entries(generatedCode.tests)) {
          const result = await this.callTool('file_create', {
            path: filePath,
            content: content as string,
          });
          testFiles.push(result);
        }
      }
      await this.reportProgress(0.9, 'テストコード生成完了');

      const duration = Date.now() - startTime;
      const result: TaskResult = {
        taskId: task.id,
        status: 'completed',
        result: {
          files,
          test_files: testFiles,
        },
        confidence: 0.85,
        metrics: {
          duration_ms: duration,
          tools_called: files.length + testFiles.length,
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
          'CODEGEN_ERROR'
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
   * コードを生成（モック実装）
   */
  private async generateCode(
    specification: string,
    language: string,
    _framework?: string
  ): Promise<{ files: Record<string, string>; tests?: Record<string, string> }> {
    // 実際の実装では、LLMを使用してコードを生成
    const fileName = `generated.${language === 'typescript' ? 'ts' : 'js'}`;
    const code = `// Generated code for: ${specification}\n\nexport function main() {\n  // Implementation here\n}\n`;

    return {
      files: {
        [fileName]: code,
      },
      tests: {
        [`${fileName}.test.${language === 'typescript' ? 'ts' : 'js'}`]: `// Test code\n`,
      },
    };
  }
}

