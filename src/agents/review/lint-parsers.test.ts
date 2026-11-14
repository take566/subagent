import { describe, it, expect } from 'vitest';
import { parseLintOutput } from './lint-parsers.js';

describe('parseLintOutput', () => {
  it('parses ESLint stylish output', () => {
    const out = [
      '/repo/src/a.ts',
      '  12:5  error    Unexpected any  @typescript-eslint/no-explicit-any',
      '  20:1  warning  Missing semicolon  semi',
      '',
      '✖ 2 problems (1 error, 1 warning)',
    ].join('\n');
    const f = parseLintOutput(out);
    expect(f).toHaveLength(2);
    expect(f[0]).toMatchObject({
      type: 'error',
      file: '/repo/src/a.ts',
      line: 12,
      column: 5,
      rule: '@typescript-eslint/no-explicit-any',
      tool: 'eslint',
      language: 'typescript',
    });
    expect(f[1]).toMatchObject({ type: 'warning', rule: 'semi' });
  });

  it('parses tsc output', () => {
    const f = parseLintOutput("src/a.ts(3,7): error TS2322: Type 'x' is not assignable.");
    expect(f[0]).toMatchObject({ file: 'src/a.ts', line: 3, column: 7, rule: 'TS2322', tool: 'tsc' });
  });

  it('parses flake8/ruff output', () => {
    const f = parseLintOutput('pkg/m.py:4:1: F401 os imported but unused\npkg/m.py:9:80: E501 line too long');
    expect(f).toHaveLength(2);
    expect(f[0]).toMatchObject({ type: 'error', rule: 'F401', language: 'python', line: 4 });
  });

  it('falls back to generic parsing', () => {
    const f = parseLintOutput('something error happened\nok');
    expect(f).toEqual([
      { type: 'error', message: 'something error happened', tool: 'generic', language: 'unknown' },
    ]);
  });
});
