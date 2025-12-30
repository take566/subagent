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
   * メッセージを検証
   */
  static validateMessage(message: A2AMessage): { valid: boolean; error?: string } {
    if (!message.message_id) {
      return { valid: false, error: 'Missing message_id' };
    }
    if (!message.timestamp) {
      return { valid: false, error: 'Missing timestamp' };
    }
    if (!message.from) {
      return { valid: false, error: 'Missing from' };
    }
    if (!message.to) {
      return { valid: false, error: 'Missing to' };
    }
    if (!message.type) {
      return { valid: false, error: 'Missing type' };
    }

    // レスポンスメッセージの検証
    if (message.type === 'task_result' || message.type === 'error') {
      const response = message as A2AResponse;
      if (!response.in_reply_to) {
        return { valid: false, error: 'Missing in_reply_to for response message' };
      }
    }

    return { valid: true };
  }

  /**
   * メッセージをシリアライズ
   */
  static serialize(message: A2AMessage): string {
    return JSON.stringify(message, null, 2);
  }

  /**
   * メッセージをデシリアライズ
   */
  static deserialize(data: string): A2AMessage {
    return JSON.parse(data) as A2AMessage;
  }
}

