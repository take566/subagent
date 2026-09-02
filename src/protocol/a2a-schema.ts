/**
 * A2A メッセージの厳密スキーマ (F-2)
 */

import { z } from 'zod';

const PrioritySchema = z.enum(['low', 'medium', 'high', 'critical']);
const TaskStatusSchema = z.enum([
  'pending',
  'running',
  'completed',
  'failed',
  'cancelled',
]);

const MessageMetadataSchema = z
  .object({
    priority: PrioritySchema.optional(),
    correlation_id: z.string().optional(),
  })
  .passthrough()
  .optional();

const ErrorInfoSchema = z.object({
  code: z.string().min(1),
  message: z.string().min(1),
  details: z.any().optional(),
  stack: z.string().optional(),
});

const TaskMetricsSchema = z
  .object({
    duration_ms: z.number(),
    tokens_used: z.number().optional(),
    tools_called: z.number().optional(),
  })
  .passthrough()
  .optional();

const TaskContextSchema = z.record(z.any()).optional();

export const TaskRequestPayloadSchema = z.object({
  task_id: z.string().min(1),
  action: z.string().min(1),
  parameters: z.record(z.any()).default({}),
  context: TaskContextSchema,
});

export const TaskResultPayloadSchema = z.object({
  task_id: z.string().min(1),
  result: z.any().optional(),
  error: ErrorInfoSchema.optional(),
});

export const ProgressPayloadSchema = z.object({
  task_id: z.string().min(1),
  progress: z.number().min(0).max(1),
  message: z.string(),
});

export const HeartbeatPayloadSchema = z
  .object({
    status: z.string().optional(),
  })
  .passthrough()
  .optional();

export const ErrorPayloadSchema = z.object({
  error: ErrorInfoSchema,
});

const BaseMessageSchema = z.object({
  message_id: z.string().min(1),
  timestamp: z.string().min(1),
  from: z.string().min(1),
  to: z.string().min(1),
  metadata: MessageMetadataSchema,
});

export const TaskRequestMessageSchema = BaseMessageSchema.extend({
  type: z.literal('task_request'),
  payload: TaskRequestPayloadSchema,
});

export const TaskResultMessageSchema = BaseMessageSchema.extend({
  type: z.literal('task_result'),
  in_reply_to: z.string().min(1),
  status: TaskStatusSchema.optional(),
  payload: TaskResultPayloadSchema,
  metrics: TaskMetricsSchema,
});

export const ProgressMessageSchema = BaseMessageSchema.extend({
  type: z.literal('progress'),
  payload: ProgressPayloadSchema,
});

export const ErrorMessageSchema = BaseMessageSchema.extend({
  type: z.literal('error'),
  in_reply_to: z.string().min(1),
  status: z.literal('failed').optional(),
  payload: ErrorPayloadSchema,
});

export const HeartbeatMessageSchema = BaseMessageSchema.extend({
  type: z.literal('heartbeat'),
  payload: HeartbeatPayloadSchema,
});

export const A2AMessageSchema = z.discriminatedUnion('type', [
  TaskRequestMessageSchema,
  TaskResultMessageSchema,
  ProgressMessageSchema,
  ErrorMessageSchema,
  HeartbeatMessageSchema,
]);

export type ParsedA2AMessage = z.infer<typeof A2AMessageSchema>;

export function parseA2AMessage(data: unknown): {
  success: true;
  data: ParsedA2AMessage;
} | {
  success: false;
  error: string;
  details: unknown;
} {
  const parsed = A2AMessageSchema.safeParse(data);
  if (!parsed.success) {
    const error = parsed.error.issues
      .map((i) => `${i.path.join('.') || '(root)'}: ${i.message}`)
      .join('; ');
    return { success: false, error, details: parsed.error.flatten() };
  }
  return { success: true, data: parsed.data };
}

/**
 * 型ガード: 未知データが有効な A2A メッセージか
 */
export function isA2AMessage(data: unknown): data is ParsedA2AMessage {
  return A2AMessageSchema.safeParse(data).success;
}
