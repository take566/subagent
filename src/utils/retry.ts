/**
 * リトライ機構実装
 */

import type { RetryPolicy } from '../types/index.js';
import { Logger } from './logger.js';

export class RetryManager {
  private policy: RetryPolicy;
  private logger: Logger;

  constructor(policy: RetryPolicy, logger: Logger) {
    this.policy = policy;
    this.logger = logger;
  }

  /**
   * バックオフ時間を計算
   */
  private calculateBackoff(attempt: number): number {
    const baseDelay = this.policy.initial_delay_ms || 1000;

    switch (this.policy.backoff) {
      case 'exponential':
        return baseDelay * Math.pow(2, attempt - 1);
      case 'linear':
        return baseDelay * attempt;
      case 'fixed':
      default:
        return baseDelay;
    }
  }

  /**
   * リトライ可能か確認
   */
  canRetry(attempt: number): boolean {
    return attempt < this.policy.max_attempts;
  }

  /**
   * 次のリトライまでの待機時間を取得
   */
  getNextDelay(attempt: number): number {
    if (!this.canRetry(attempt)) {
      return 0;
    }
    return this.calculateBackoff(attempt);
  }

  /**
   * リトライ付きで関数を実行
   */
  async executeWithRetry<T>(
    fn: () => Promise<T>,
    onRetry?: (attempt: number, error: Error) => void
  ): Promise<T> {
    let lastError: Error | null = null;

    for (let attempt = 1; attempt <= this.policy.max_attempts; attempt++) {
      try {
        return await fn();
      } catch (error) {
        lastError = error instanceof Error ? error : new Error(String(error));

        if (onRetry) {
          onRetry(attempt, lastError);
        }

        if (!this.canRetry(attempt)) {
          this.logger.warn(`Max retries reached (${this.policy.max_attempts})`);
          throw lastError;
        }

        const delay = this.getNextDelay(attempt);
        this.logger.info(`Retrying after ${delay}ms (attempt ${attempt}/${this.policy.max_attempts})`);
        await this.sleep(delay);
      }
    }

    throw lastError || new Error('Retry failed');
  }

  private sleep(ms: number): Promise<void> {
    return new Promise((resolve) => setTimeout(resolve, ms));
  }
}

