import { describe, it, expect } from 'vitest';
import {
  validateSubagentConfig,
  validateTask,
  validateTaskAgainstInterface,
  ValidationError,
} from './validation.js';
import type { SubagentConfig, Task } from './index.js';

const validConfig: SubagentConfig = {
  id: 'test-001',
  name: 'TestAgent',
  version: '1.0.0',
  role: {
    description: 'test',
    capabilities: ['test'],
  },
  interface: {
    input: [
      { name: 'query', type: 'string', required: true },
      { name: 'depth', type: 'string', required: false },
    ],
    output: [{ name: 'result', type: 'object', required: true }],
  },
  tools: ['web_search'],
  behavior: {
    timeout: 5000,
    retry_policy: { max_attempts: 3, backoff: 'exponential' },
  },
};

describe('validateSubagentConfig', () => {
  it('accepts a valid config', () => {
    expect(validateSubagentConfig(validConfig).id).toBe('test-001');
  });

  it('rejects missing id', () => {
    const bad = { ...validConfig, id: '' };
    expect(() => validateSubagentConfig(bad)).toThrow(ValidationError);
  });

  it('rejects invalid backoff', () => {
    const bad = {
      ...validConfig,
      behavior: {
        retry_policy: { max_attempts: 1, backoff: 'nope' },
      },
    };
    expect(() => validateSubagentConfig(bad)).toThrow(ValidationError);
  });
});

describe('validateTaskAgainstInterface', () => {
  it('passes when required params present', () => {
    const task: Task = {
      id: 't1',
      action: 'research',
      parameters: { query: 'hello' },
    };
    expect(() => validateTaskAgainstInterface(task, validConfig)).not.toThrow();
  });

  it('fails when required param missing', () => {
    const task: Task = {
      id: 't1',
      action: 'research',
      parameters: {},
    };
    expect(() => validateTaskAgainstInterface(task, validConfig)).toThrow(
      /missing required: query/
    );
  });

  it('fails on type mismatch', () => {
    const task: Task = {
      id: 't1',
      action: 'research',
      parameters: { query: 123 },
    };
    expect(() => validateTaskAgainstInterface(task, validConfig)).toThrow(
      /type mismatch/
    );
  });
});

describe('validateTask', () => {
  it('rejects empty action', () => {
    expect(() =>
      validateTask({ id: 't', action: '', parameters: {} })
    ).toThrow(ValidationError);
  });
});
