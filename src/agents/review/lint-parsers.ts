/**
 * Lint出力の言語別パーサ
 * bash経由で得た lint 出力を正規化された findings に変換する
 */

export type LintLanguage = 'typescript' | 'python' | 'unknown';

/** 正規化された finding (type/message は既存の issues 形式と互換) */
export interface LintFinding {
  type: 'error' | 'warning';
  message: string;
  file?: string;
  line?: number;
  column?: number;
  rule?: string;
  tool: string;
  language: LintLanguage;
}

// ESLint stylish: "  12:5  error  message  rule-name"
const ESLINT_ROW = /^\s+(\d+):(\d+)\s+(error|warning)\s+(.*?)(?:\s{2,}(\S+))?\s*$/;
// tsc: "src/a.ts(12,5): error TS2322: message"
const TSC_ROW = /^(.+?)\((\d+),(\d+)\): (error|warning) (TS\d+): (.*)$/;
// flake8 / ruff concise: "a.py:12:5: E501 message"
const PY_ROW = /^(.+?):(\d+):(\d+): ([A-Z]+\d+) (.*)$/;

/** 先頭文字が W/C/N/D の flake8/ruff コードは warning、それ以外は error とみなす */
function pythonSeverity(code: string): 'error' | 'warning' {
  return /^[WCND]/.test(code) ? 'warning' : 'error';
}

export function parseEslintOutput(output: string): LintFinding[] {
  const findings: LintFinding[] = [];
  let file: string | undefined;
  for (const line of output.split('\n')) {
    const tsc = TSC_ROW.exec(line);
    if (tsc) {
      findings.push({
        type: tsc[4] as 'error' | 'warning',
        message: tsc[6],
        file: tsc[1],
        line: Number(tsc[2]),
        column: Number(tsc[3]),
        rule: tsc[5],
        tool: 'tsc',
        language: 'typescript',
      });
      continue;
    }
    const row = ESLINT_ROW.exec(line);
    if (row) {
      findings.push({
        type: row[3] as 'error' | 'warning',
        message: row[4],
        file,
        line: Number(row[1]),
        column: Number(row[2]),
        rule: row[5],
        tool: 'eslint',
        language: 'typescript',
      });
    } else if (line.trim() && !/^\s/.test(line) && !line.startsWith('✖')) {
      file = line.trim();
    }
  }
  return findings;
}

export function parsePythonOutput(output: string): LintFinding[] {
  const findings: LintFinding[] = [];
  for (const line of output.split('\n')) {
    const m = PY_ROW.exec(line);
    if (!m) continue;
    findings.push({
      type: pythonSeverity(m[4]),
      message: m[5],
      file: m[1],
      line: Number(m[2]),
      column: Number(m[3]),
      rule: m[4],
      tool: 'python-lint',
      language: 'python',
    });
  }
  return findings;
}

/** 従来の行単位フォールバック */
function parseGeneric(output: string): LintFinding[] {
  return output
    .split('\n')
    .filter((l) => l.includes('error') || l.includes('warning'))
    .map((l) => ({
      type: l.includes('error') ? ('error' as const) : ('warning' as const),
      message: l,
      tool: 'generic',
      language: 'unknown' as const,
    }));
}

/** 出力形式から言語を推定してパースする。どれにも当たらなければ汎用パーサ */
export function parseLintOutput(output: string): LintFinding[] {
  const parsers = [parseEslintOutput, parsePythonOutput];
  for (const parse of parsers) {
    const found = parse(output);
    if (found.length > 0) return found;
  }
  return parseGeneric(output);
}
