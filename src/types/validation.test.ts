import { describe, it, expect } from 'vitest';
import { validateSubagentConfig, validateTask, ValidationError } from './validation.js';
import type { SubagentConfig } from './index.js';

const config: SubagentConfig = {
  id: 'a',
  name: 'A',
  version: '1.0.0',
  role: { description: 'd', capabilities: [] },
  interface: { input: [{ name: 'query', type: 'string', required: true }], output: [] },
  tools: [],
  behavior: {},
};

describe('validation', () => {
  it('accepts valid config and task', () => {
    expect(validateSubagentConfig(config)).toBe(config);
    expect(() => validateTask({ id: '1', action: 'x', parameters: { query: 'q' } }, config)).not.toThrow();
  });
  it('rejects config missing fields', () => {
    expect(() => validateSubagentConfig({ ...config, id: undefined })).toThrow(ValidationError);
  });
  it('rejects task missing required input', () => {
    expect(() => validateTask({ id: '1', action: 'x', parameters: {} }, config)).toThrow(/parameters.query/);
  });
  it('rejects malformed task', () => {
    expect(() => validateTask({ action: 'x' })).toThrow(ValidationError);
  });
});
