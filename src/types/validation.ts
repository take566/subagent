/**
 * zod による実行時バリデーション (SubagentConfig / Task)
 */

import { z } from 'zod';
import type { SubagentConfig, Task } from './index.js';

export class ValidationError extends Error {
  readonly issues: string[];

  constructor(subject: string, issues: string[]) {
    super(`${subject} の検証に失敗: ${issues.join('; ')}`);
    this.name = 'ValidationError';
    this.issues = issues;
  }
}

const parameterDefinitionSchema = z.object({
  name: z.string().min(1),
  type: z.string().min(1),
  required: z.boolean(),
  schema: z.any().optional(),
});

export const subagentConfigSchema = z.object({
  id: z.string().min(1),
  name: z.string().min(1),
  version: z.string().min(1),
  role: z.object({
    description: z.string(),
    capabilities: z.array(z.string()),
    constraints: z.record(z.any()).optional(),
  }),
  interface: z.object({
    input: z.array(parameterDefinitionSchema),
    output: z.array(parameterDefinitionSchema),
  }),
  tools: z.array(z.string()),
  behavior: z
    .object({
      retry_policy: z
        .object({
          max_attempts: z.number().int().min(0),
          backoff: z.enum(['exponential', 'linear', 'fixed']),
          initial_delay_ms: z.number().min(0).optional(),
        })
        .optional(),
      timeout: z.number().positive().optional(),
      fallback: z.string().optional(),
    })
    .passthrough(),
});

export const taskSchema = z
  .object({
    id: z.string().min(1),
    action: z.string().min(1),
    parameters: z.record(z.any()),
    context: z.record(z.any()).optional(),
    metadata: z.record(z.any()).optional(),
  })
  .passthrough();

function formatIssues(error: z.ZodError): string[] {
  return error.issues.map((i) => `${i.path.join('.') || '(root)'}: ${i.message}`);
}

export function validateSubagentConfig(config: unknown): SubagentConfig {
  const parsed = subagentConfigSchema.safeParse(config);
  if (!parsed.success) {
    throw new ValidationError('SubagentConfig', formatIssues(parsed.error));
  }
  return config as SubagentConfig;
}

/**
 * Task の形と、config.interface.input の必須項目を検証する
 */
export function validateTask(task: unknown, config?: SubagentConfig): Task {
  const parsed = taskSchema.safeParse(task);
  if (!parsed.success) {
    throw new ValidationError('Task', formatIssues(parsed.error));
  }
  const t = task as Task;
  if (config) {
    const missing = config.interface.input
      .filter((p) => p.required && (t.parameters[p.name] === undefined || t.parameters[p.name] === null))
      .map((p) => `parameters.${p.name}: 必須項目がありません`);
    if (missing.length > 0) {
      throw new ValidationError(`Task ${t.id}`, missing);
    }
  }
  return t;
}
