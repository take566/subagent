/**
 * エラーハンドリング実装
 */

import type {
  ErrorHandlingStrategy,
  TaskResult,
  ErrorInfo,
} from '../types/index.js';
import { Logger } from './logger.js';

export class ErrorHandler {
  private strategies: ErrorHandlingStrategy;
  private logger: Logger;

  constructor(strategies: ErrorHandlingStrategy, logger: Logger) {
    this.strategies = strategies;
    this.logger = logger;
  }

  /**
   * タイムアウトエラーを処理
   */
  async handleTimeout(
    _error: Error,
    retryCount: number,
    maxRetries: number = 3
  ): Promise<{ shouldRetry: boolean; action: string }> {
    const strategy = this.strategies.timeout;
    if (!strategy) {
      return { shouldRetry: false, action: 'fail' };
    }

    if (retryCount >= maxRetries) {
      this.logger.warn('Max retries reached for timeout', { retryCount });
      return { shouldRetry: false, action: strategy.fallback || 'fail' };
    }

    return { shouldRetry: true, action: strategy.action };
  }

  /**
   * バリデーションエラーを処理
   */
  async handleValidationError(
    _error: Error,
    attemptCount: number
  ): Promise<{ action: string; shouldEscalate: boolean }> {
    const strategy = this.strategies.validation_error;
    if (!strategy) {
      return { action: 'fail', shouldEscalate: false };
    }

    const shouldEscalate = strategy.escalate_after
      ? attemptCount >= strategy.escalate_after
      : false;

    return {
      action: strategy.action,
      shouldEscalate,
    };
  }

  /**
   * ツール失敗を処理
   */
  async handleToolFailure(
    toolName: string,
    _error: Error
  ): Promise<{ useAlternative: boolean; alternativeTool?: string }> {
    const strategy = this.strategies.tool_failure;
    if (!strategy || !strategy.alternatives) {
      return { useAlternative: false };
    }

    const alternative = strategy.alternatives.find((alt) => alt.primary === toolName);
    if (alternative) {
      this.logger.info(`Switching from ${toolName} to ${alternative.backup}`);
      return {
        useAlternative: true,
        alternativeTool: alternative.backup,
      };
    }

    return { useAlternative: false };
  }

  /**
   * 部分的な失敗を処理
   */
  async handlePartialFailure(
    _result: TaskResult,
    _error: Error
  ): Promise<{ returnPartial: boolean; notify: boolean }> {
    const strategy = this.strategies.partial_failure;
    if (!strategy) {
      return { returnPartial: false, notify: false };
    }

    return {
      returnPartial: strategy.action === 'return_partial_with_flag',
      notify: strategy.notify || false,
    };
  }

  /**
   * エラー情報を作成
   */
  createErrorInfo(error: Error, code: string = 'UNKNOWN_ERROR'): ErrorInfo {
    return {
      code,
      message: error.message,
      details: {
        name: error.name,
        stack: error.stack,
      },
      stack: error.stack,
    };
  }

  /**
   * エスカレーションが必要か判定
   */
  shouldEscalate(result: TaskResult, conditions: string[]): boolean {
    for (const condition of conditions) {
      if (condition === 'confidence < 0.5' && (result.confidence || 1.0) < 0.5) {
        return true;
      }
      if (condition === 'critical_error' && result.error) {
        return true;
      }
      if (condition === 'requires_approval') {
        // 実装に応じて判定ロジックを追加
        return false;
      }
    }
    return false;
  }
}

