/**
 * A2A (Agent-to-Agent) 通信プロトコル実装
 */

import { v4 as uuidv4 } from 'uuid';
import type {
  A2AMessage,
  A2AResponse,
  MessageType,
  TaskRequestPayload,
  TaskResultPayload,
  TaskStatus,
  TaskMetrics,
  Priority,
} from '../types/index.js';

const MESSAGE_TYPES: MessageType[] = ['task_request', 'task_result', 'progress', 'error', 'heartbeat'];
const TASK_STATUSES: TaskStatus[] = ['pending', 'running', 'completed', 'failed', 'cancelled'];

function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null && !Array.isArray(v);
}

function isNonEmptyString(v: unknown): v is string {
  return typeof v === 'string' && v.length > 0;
}

/**
 * 種別ごとのペイロード検証。問題があればエラー文字列、なければ undefined
 */
function validatePayload(type: MessageType, payload: unknown): string | undefined {
  if (!isObject(payload)) {
    return `payload must be an object for ${type}`;
  }
  switch (type) {
    case 'task_request':
      if (!isNonEmptyString(payload.task_id)) return 'task_request payload: task_id must be a non-empty string';
      if (!isNonEmptyString(payload.action)) return 'task_request payload: action must be a non-empty string';
      if (!isObject(payload.parameters)) return 'task_request payload: parameters must be an object';
      if (payload.context !== undefined && !isObject(payload.context)) {
        return 'task_request payload: context must be an object';
      }
      return undefined;
    case 'task_result':
      if (!isNonEmptyString(payload.task_id)) return 'task_result payload: task_id must be a non-empty string';
      if (payload.error !== undefined && !isErrorInfo(payload.error)) {
        return 'task_result payload: error must have string code and message';
      }
      return undefined;
    case 'progress':
      if (!isNonEmptyString(payload.task_id)) return 'progress payload: task_id must be a non-empty string';
      if (typeof payload.progress !== 'number' || !Number.isFinite(payload.progress)) {
        return 'progress payload: progress must be a finite number';
      }
      if (typeof payload.message !== 'string') return 'progress payload: message must be a string';
      return undefined;
    case 'error':
      if (!isErrorInfo(payload.error)) return 'error payload: error must have string code and message';
      return undefined;
    case 'heartbeat':
      if (payload.status !== 'alive') return "heartbeat payload: status must be 'alive'";
      return undefined;
  }
}

function isErrorInfo(v: unknown): boolean {
  return isObject(v) && typeof v.code === 'string' && typeof v.message === 'string';
}

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
  static validateMessage(message: unknown): { valid: boolean; error?: string } {
    if (!isObject(message)) {
      return { valid: false, error: 'Message must be an object' };
    }
    for (const key of ['message_id', 'timestamp', 'from', 'to'] as const) {
      if (message[key] === undefined || message[key] === null || message[key] === '') {
        return { valid: false, error: `Missing ${key}` };
      }
      if (!isNonEmptyString(message[key])) {
        return { valid: false, error: `${key} must be a non-empty string` };
      }
    }
    if (Number.isNaN(Date.parse(message.timestamp as string))) {
      return { valid: false, error: 'timestamp must be a valid date string' };
    }
    if (message.type === undefined || message.type === null || message.type === '') {
      return { valid: false, error: 'Missing type' };
    }
    const type = message.type;
    if (typeof type !== 'string' || !MESSAGE_TYPES.includes(type as MessageType)) {
      return { valid: false, error: `Unknown message type: ${String(type)}` };
    }

    // レスポンスメッセージの検証
    if (type === 'task_result' || type === 'error') {
      if (!isNonEmptyString(message.in_reply_to)) {
        return { valid: false, error: 'Missing in_reply_to for response message' };
      }
      if (message.status !== undefined && !TASK_STATUSES.includes(message.status as TaskStatus)) {
        return { valid: false, error: 'Invalid status' };
      }
    }

    if (message.metadata !== undefined && !isObject(message.metadata)) {
      return { valid: false, error: 'metadata must be an object' };
    }

    const payloadError = validatePayload(type as MessageType, message.payload);
    if (payloadError) {
      return { valid: false, error: payloadError };
    }

    return { valid: true };
  }

  /**
   * 型ガード: 検証を通った A2AMessage か
   */
  static isMessage(value: unknown): value is A2AMessage {
    return A2AProtocol.validateMessage(value).valid;
  }

  /**
   * メッセージをシリアライズ (不正なメッセージは throw)
   */
  static serialize(message: A2AMessage): string {
    const result = A2AProtocol.validateMessage(message);
    if (!result.valid) {
      throw new Error(`Invalid A2A message: ${result.error}`);
    }
    return JSON.stringify(message, null, 2);
  }

  /**
   * メッセージをデシリアライズ (不正な JSON / スキーマ違反は throw)
   */
  static deserialize(data: string): A2AMessage {
    let parsed: unknown;
    try {
      parsed = JSON.parse(data);
    } catch {
      throw new Error('Invalid A2A message: malformed JSON');
    }
    const result = A2AProtocol.validateMessage(parsed);
    if (!result.valid) {
      throw new Error(`Invalid A2A message: ${result.error}`);
    }
    return parsed as A2AMessage;
  }
}

