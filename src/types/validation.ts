/**
 * Zod による実行時バリデーション (F-1)
 */

import { z } from 'zod';
import type { SubagentConfig, Task } from './index.js';

export class ValidationError extends Error {
  readonly code = 'VALIDATION_ERROR';
  readonly details: unknown;

  constructor(message: string, details?: unknown) {
    super(message);
    this.name = 'ValidationError';
    this.details = details;
  }
}

const PrioritySchema = z.enum(['low', 'medium', 'high', 'critical']);

const ParameterDefinitionSchema = z.object({
  name: z.string().min(1),
  type: z.string().min(1),
  required: z.boolean(),
  schema: z.any().optional(),
});

const RetryPolicySchema = z.object({
  max_attempts: z.number().int().positive(),
  backoff: z.enum(['exponential', 'linear', 'fixed']),
  initial_delay_ms: z.number().nonnegative().optional(),
});

export const SubagentConfigSchema = z.object({
  id: z.string().min(1),
  name: z.string().min(1),
  version: z.string().min(1),
  role: z.object({
    description: z.string(),
    capabilities: z.array(z.string()),
    constraints: z.record(z.any()).optional(),
  }),
  interface: z.object({
    input: z.array(ParameterDefinitionSchema),
    output: z.array(ParameterDefinitionSchema),
  }),
  tools: z.array(z.string()),
  behavior: z.object({
    retry_policy: RetryPolicySchema.optional(),
    timeout: z.number().positive().optional(),
    fallback: z.string().optional(),
  }),
});

export const TaskSchema = z.object({
  id: z.string().min(1),
  action: z.string().min(1),
  parameters: z.record(z.any()),
  context: z.record(z.any()).optional(),
  metadata: z
    .object({
      priority: PrioritySchema.optional(),
      correlation_id: z.string().optional(),
      retry_count: z.number().int().nonnegative().optional(),
      created_at: z.string().optional(),
    })
    .passthrough()
    .optional(),
});

function formatZodError(error: z.ZodError): string {
  return error.issues
    .map((issue) => `${issue.path.join('.') || '(root)'}: ${issue.message}`)
    .join('; ');
}

/**
 * SubagentConfig を検証する
 */
export function validateSubagentConfig(config: unknown): SubagentConfig {
  const parsed = SubagentConfigSchema.safeParse(config);
  if (!parsed.success) {
    throw new ValidationError(
      `Invalid SubagentConfig: ${formatZodError(parsed.error)}`,
      parsed.error.flatten()
    );
  }
  return parsed.data as SubagentConfig;
}

/**
 * Task の基本構造を検証する
 */
export function validateTask(task: unknown): Task {
  const parsed = TaskSchema.safeParse(task);
  if (!parsed.success) {
    throw new ValidationError(
      `Invalid Task: ${formatZodError(parsed.error)}`,
      parsed.error.flatten()
    );
  }
  return parsed.data as Task;
}

/**
 * Subagent interface.input 定義に対して Task.parameters を検証する
 */
export function validateTaskAgainstInterface(
  task: Task,
  config: SubagentConfig
): void {
  validateTask(task);

  const missing: string[] = [];
  const typeMismatches: string[] = [];

  for (const def of config.interface.input) {
    const value = task.parameters[def.name];
    const isMissing = value === undefined || value === null;

    if (def.required && isMissing) {
      missing.push(def.name);
      continue;
    }
    if (isMissing) continue;

    if (!matchesDeclaredType(value, def.type)) {
      typeMismatches.push(
        `${def.name} expected ${def.type}, got ${typeof value}`
      );
    }
  }

  if (missing.length > 0 || typeMismatches.length > 0) {
    const parts: string[] = [];
    if (missing.length > 0) {
      parts.push(`missing required: ${missing.join(', ')}`);
    }
    if (typeMismatches.length > 0) {
      parts.push(`type mismatch: ${typeMismatches.join('; ')}`);
    }
    throw new ValidationError(
      `Task parameters invalid for ${config.name}: ${parts.join(' | ')}`,
      { missing, typeMismatches, agentId: config.id }
    );
  }
}

function matchesDeclaredType(value: unknown, type: string): boolean {
  switch (type.toLowerCase()) {
    case 'string':
      return typeof value === 'string';
    case 'number':
      return typeof value === 'number' && !Number.isNaN(value);
    case 'boolean':
      return typeof value === 'boolean';
    case 'array':
      return Array.isArray(value);
    case 'object':
      return typeof value === 'object' && value !== null && !Array.isArray(value);
    case 'any':
      return true;
    default:
      // カスタム型名は object 相当として許容
      return value !== undefined;
  }
}
