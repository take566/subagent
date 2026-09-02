/**
 * A2A (Agent-to-Agent) 通信プロトコル実装
 */

import { v4 as uuidv4 } from 'uuid';
import type {
  A2AMessage,
  A2AResponse,
  TaskRequestPayload,
  TaskResultPayload,
  TaskStatus,
  TaskMetrics,
  Priority,
} from '../types/index.js';
import { parseA2AMessage, isA2AMessage } from './a2a-schema.js';

export { parseA2AMessage, isA2AMessage, A2AMessageSchema } from './a2a-schema.js';

export class A2AProtocol {
  /**
   * タスクリクエストメッセージを作成
   */
  static createTaskRequest(
    from: string,
    to: string,
    payload: TaskRequestPayload,
    metadata?: { priority?: Priority; correlation_id?: string }
  ): A2AMessage {
    return {
      message_id: uuidv4(),
      timestamp: new Date().toISOString(),
      from,
      to,
      type: 'task_request',
      payload,
      metadata: {
        priority: metadata?.priority || 'medium',
        correlation_id: metadata?.correlation_id,
      },
    };
  }

  /**
   * タスク結果レスポンスを作成
   */
  static createTaskResult(
    from: string,
    to: string,
    inReplyTo: string,
    payload: TaskResultPayload,
    status: TaskStatus,
    metrics?: TaskMetrics
  ): A2AResponse {
    return {
      message_id: uuidv4(),
      in_reply_to: inReplyTo,
      timestamp: new Date().toISOString(),
      from,
      to,
      type: 'task_result',
      status,
      payload,
      metrics,
    };
  }

  /**
   * プログレスメッセージを作成
   */
  static createProgressMessage(
    from: string,
    to: string,
    taskId: string,
    progress: number,
    message: string
  ): A2AMessage {
    return {
      message_id: uuidv4(),
      timestamp: new Date().toISOString(),
      from,
      to,
      type: 'progress',
      payload: {
        task_id: taskId,
        progress,
        message,
      },
    };
  }

  /**
   * エラーメッセージを作成
   */
  static createErrorMessage(
    from: string,
    to: string,
    inReplyTo: string,
    error: { code: string; message: string; details?: any }
  ): A2AResponse {
    return {
      message_id: uuidv4(),
      in_reply_to: inReplyTo,
      timestamp: new Date().toISOString(),
      from,
      to,
      type: 'error',
      status: 'failed',
      payload: {
        error,
      },
    };
  }

  /**
   * ハートビートメッセージを作成
   */
  static createHeartbeat(from: string, to: string): A2AMessage {
    return {
      message_id: uuidv4(),
      timestamp: new Date().toISOString(),
      from,
      to,
      type: 'heartbeat',
      payload: {
        status: 'alive',
      },
    };
  }

  /**
   * メッセージを検証（厳密スキーマ）
   */
  static validateMessage(
    message: unknown
  ): { valid: boolean; error?: string; details?: unknown } {
    const parsed = parseA2AMessage(message);
    if (!parsed.success) {
      return { valid: false, error: parsed.error, details: parsed.details };
    }
    return { valid: true };
  }

  /**
   * 型ガード
   */
  static isMessage(data: unknown): data is A2AMessage {
    return isA2AMessage(data);
  }

  /**
   * メッセージをシリアライズ
   */
  static serialize(message: A2AMessage): string {
    const validation = this.validateMessage(message);
    if (!validation.valid) {
      throw new Error(`Cannot serialize invalid A2A message: ${validation.error}`);
    }
    return JSON.stringify(message);
  }

  /**
   * メッセージをデシリアライズし、スキーマ検証する
   */
  static deserialize(data: string): A2AMessage {
    let raw: unknown;
    try {
      raw = JSON.parse(data);
    } catch {
      throw new Error('Invalid JSON for A2A message');
    }
    const parsed = parseA2AMessage(raw);
    if (!parsed.success) {
      throw new Error(`Invalid A2A message: ${parsed.error}`);
    }
    return parsed.data as A2AMessage;
  }
}

