import { describe, it, expect } from 'vitest';
import { evaluateCondition as ev } from './condition-evaluator.js';
import type { TaskResult } from '../types/index.js';

const ok = { taskId: 'a', status: 'completed', result: { n: 3 } } as TaskResult;
const ng = { taskId: 'b', status: 'failed' } as TaskResult;

describe('evaluateCondition', () => {
  it('previous.success', () => {
    expect(ev('previous.success', [])).toBe(true);
    expect(ev('previous.success', [ok])).toBe(true);
    expect(ev('previous.success', [ok, ng])).toBe(false);
    expect(ev('!previous.success', [ok, ng])).toBe(true);
  });
  it('comparison and refs', () => {
    expect(ev('previous.results.length >= 2', [ok, ng])).toBe(true);
    expect(ev('previous.results.length == 3', [ok, ng])).toBe(false);
    expect(ev("previous.last.status === 'failed'", [ok, ng])).toBe(true);
    expect(ev('previous.results[0].result.n > 2', [ok])).toBe(true);
  });
  it('and/or/not/parens', () => {
    expect(ev('previous.success and previous.results.length > 0', [ok])).toBe(true);
    expect(ev('not previous.success or false', [ok, ng])).toBe(true);
    expect(ev('(true || false) && !false', [])).toBe(true);
    expect(ev('true && false', [])).toBe(false);
  });
  it('rejects unsafe or invalid expressions as false', () => {
    for (const e of [
      'process.exit(1)',
      'previous.constructor',
      'previous.__proto__',
      'previous.results.constructor.constructor("return 1")()',
      'true &&',
      '1 +',
      '(true',
      '',
      'previous.results.length > "a"',
    ]) {
      expect(ev(e, [ok])).toBe(false);
    }
  });
});
