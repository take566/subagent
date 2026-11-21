/**
 * 安全な条件式評価（再帰下降パーサ）
 *
 * eval / Function / vm は使わない。許可された構文以外・評価エラーは false。
 *
 * 文法:
 *   or      := and (('||' | 'or') and)*
 *   and     := not (('&&' | 'and') not)*
 *   not     := ('!' | 'not') not | cmp
 *   cmp     := primary (('=='|'==='|'!='|'!=='|'<'|'>'|'<='|'>=') primary)?
 *   primary := number | string | true | false | null | '(' or ')' | ref
 *   ref     := 'previous' ('.' ident | '[' number ']')*
 */

import type { TaskResult } from '../types/index.js';

type Token = { type: 'num' | 'str' | 'id' | 'op'; value: string };

const OPS = ['===', '!==', '==', '!=', '<=', '>=', '&&', '||', '<', '>', '!', '(', ')', '[', ']', '.'];
const MAX_LENGTH = 500;
const MAX_DEPTH = 50;
const FORBIDDEN_KEYS = new Set(['__proto__', 'constructor', 'prototype']);

function tokenize(src: string): Token[] {
  const tokens: Token[] = [];
  let i = 0;
  while (i < src.length) {
    const c = src[i];
    if (/\s/.test(c)) {
      i++;
    } else if (/[0-9]/.test(c)) {
      const m = /^[0-9]+(\.[0-9]+)?/.exec(src.slice(i))!;
      tokens.push({ type: 'num', value: m[0] });
      i += m[0].length;
    } else if (/[A-Za-z_]/.test(c)) {
      const m = /^[A-Za-z_][A-Za-z0-9_]*/.exec(src.slice(i))!;
      tokens.push({ type: 'id', value: m[0] });
      i += m[0].length;
    } else if (c === '"' || c === "'") {
      const end = src.indexOf(c, i + 1);
      if (end < 0) throw new Error('unterminated string');
      tokens.push({ type: 'str', value: src.slice(i + 1, end) });
      i = end + 1;
    } else {
      const op = OPS.find((o) => src.startsWith(o, i));
      if (!op) throw new Error(`unexpected character: ${c}`);
      tokens.push({ type: 'op', value: op });
      i += op.length;
    }
  }
  return tokens;
}

class Parser {
  private pos = 0;
  private depth = 0;

  constructor(private tokens: Token[], private scope: Record<string, unknown>) {}

  parse(): unknown {
    const v = this.or();
    if (this.pos < this.tokens.length) throw new Error('unexpected token');
    return v;
  }

  private peek(): Token | undefined {
    return this.tokens[this.pos];
  }

  private isOp(...values: string[]): boolean {
    const t = this.peek();
    return !!t && t.type === 'op' && values.includes(t.value);
  }

  private isWord(word: string): boolean {
    const t = this.peek();
    return !!t && t.type === 'id' && t.value === word;
  }

  private or(): unknown {
    let left = this.and();
    while (this.isOp('||') || this.isWord('or')) {
      this.pos++;
      const right = this.and();
      left = Boolean(left) || Boolean(right);
    }
    return left;
  }

  private and(): unknown {
    let left = this.not();
    while (this.isOp('&&') || this.isWord('and')) {
      this.pos++;
      const right = this.not();
      left = Boolean(left) && Boolean(right);
    }
    return left;
  }

  private not(): unknown {
    if (this.isOp('!') || this.isWord('not')) {
      this.pos++;
      if (++this.depth > MAX_DEPTH) throw new Error('too deep');
      const v = !this.not();
      this.depth--;
      return v;
    }
    return this.cmp();
  }

  private cmp(): unknown {
    const left = this.primary();
    if (this.isOp('==', '===', '!=', '!==', '<', '>', '<=', '>=')) {
      const op = this.tokens[this.pos++].value;
      const right = this.primary();
      switch (op) {
        case '==':
        case '===':
          return left === right;
        case '!=':
        case '!==':
          return left !== right;
      }
      if (typeof left !== typeof right || (typeof left !== 'number' && typeof left !== 'string')) {
        return false;
      }
      const l = left as number | string;
      const r = right as number | string;
      return op === '<' ? l < r : op === '>' ? l > r : op === '<=' ? l <= r : l >= r;
    }
    return left;
  }

  private primary(): unknown {
    const t = this.tokens[this.pos++];
    if (!t) throw new Error('unexpected end');
    if (t.type === 'num') return Number(t.value);
    if (t.type === 'str') return t.value;
    if (t.type === 'op' && t.value === '(') {
      if (++this.depth > MAX_DEPTH) throw new Error('too deep');
      const v = this.or();
      this.depth--;
      if (!this.isOp(')')) throw new Error('expected )');
      this.pos++;
      return v;
    }
    if (t.type === 'id') {
      if (t.value === 'true') return true;
      if (t.value === 'false') return false;
      if (t.value === 'null') return null;
      if (!Object.prototype.hasOwnProperty.call(this.scope, t.value)) {
        throw new Error(`unknown identifier: ${t.value}`);
      }
      let value: unknown = this.scope[t.value];
      while (this.isOp('.', '[')) {
        const op = this.tokens[this.pos++].value;
        const k = this.tokens[this.pos++];
        let key: string | number;
        if (op === '.') {
          if (!k || k.type !== 'id') throw new Error('expected property name');
          key = k.value;
        } else {
          if (!k || k.type !== 'num') throw new Error('expected index');
          key = Number(k.value);
          if (!this.isOp(']')) throw new Error('expected ]');
          this.pos++;
        }
        value = this.member(value, key);
      }
      return value;
    }
    throw new Error(`unexpected token: ${t.value}`);
  }

  private member(obj: unknown, key: string | number): unknown {
    if (typeof key === 'string' && FORBIDDEN_KEYS.has(key)) throw new Error('forbidden key');
    if (obj === null || typeof obj !== 'object') return undefined;
    if (Array.isArray(obj) && key === 'length') return obj.length;
    if (!Object.prototype.hasOwnProperty.call(obj, key)) return undefined;
    return (obj as Record<string | number, unknown>)[key];
  }
}

/**
 * 条件式を評価する。構文エラー・未許可の構文・評価エラーは false。
 */
export function evaluateCondition(condition: string, previousResults: TaskResult[]): boolean {
  if (typeof condition !== 'string' || condition.length > MAX_LENGTH) return false;
  try {
    const previous = {
      success: previousResults.every((r) => r.status === 'completed'),
      results: previousResults,
      last: previousResults[previousResults.length - 1],
    };
    return Boolean(new Parser(tokenize(condition), { previous }).parse());
  } catch {
    return false;
  }
}
