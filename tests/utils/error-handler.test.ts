import { describe, it, expect } from 'vitest';
import { ErrorHandler } from '../../src/utils/error-handler';
import { Logger } from '../../src/utils/logger';
import type { ErrorHandlingStrategy, TaskResult } from '../../src/types/index';

const mk = (s: ErrorHandlingStrategy = {}) => new ErrorHandler(s, new Logger('t'));
const err = new Error('x');
const res = (o: Partial<TaskResult> = {}) => ({ ...o }) as TaskResult;

describe('handleTimeout', () => {
  it('fails when no strategy configured', async () => {
    expect(await mk().handleTimeout(err, 0)).toEqual({ shouldRetry: false, action: 'fail' });
  });

  it('retries below max retries', async () => {
    const h = mk({ timeout: { action: 'retry', fallback: 'skip' } });
    expect(await h.handleTimeout(err, 2)).toEqual({ shouldRetry: true, action: 'retry' });
  });

  it('uses fallback at max retries, default max is 3', async () => {
    const h = mk({ timeout: { action: 'retry', fallback: 'skip' } });
    expect(await h.handleTimeout(err, 3)).toEqual({ shouldRetry: false, action: 'skip' });
    expect(await h.handleTimeout(err, 1, 1)).toEqual({ shouldRetry: false, action: 'skip' });
  });

  it('falls back to "fail" when no fallback defined', async () => {
    const h = mk({ timeout: { action: 'retry' } });
    expect(await h.handleTimeout(err, 5)).toEqual({ shouldRetry: false, action: 'fail' });
  });
});

describe('handleValidationError', () => {
  it('fails without strategy', async () => {
    expect(await mk().handleValidationError(err, 1)).toEqual({ action: 'fail', shouldEscalate: false });
  });

  it('escalates at or after escalate_after', async () => {
    const h = mk({ validation_error: { action: 'retry', escalate_after: 2 } });
    expect((await h.handleValidationError(err, 1)).shouldEscalate).toBe(false);
    expect(await h.handleValidationError(err, 2)).toEqual({ action: 'retry', shouldEscalate: true });
  });

  it('never escalates when escalate_after unset', async () => {
    const h = mk({ validation_error: { action: 'retry' } });
    expect((await h.handleValidationError(err, 99)).shouldEscalate).toBe(false);
  });
});

describe('handleToolFailure', () => {
  it('no alternative without strategy or alternatives', async () => {
    expect(await mk().handleToolFailure('a', err)).toEqual({ useAlternative: false });
    expect(await mk({ tool_failure: { action: 'x' } }).handleToolFailure('a', err)).toEqual({ useAlternative: false });
  });

  it('switches to backup for matching primary only', async () => {
    const h = mk({ tool_failure: { action: 'switch', alternatives: [{ primary: 'a', backup: 'b' }] } });
    expect(await h.handleToolFailure('a', err)).toEqual({ useAlternative: true, alternativeTool: 'b' });
    expect(await h.handleToolFailure('z', err)).toEqual({ useAlternative: false });
  });
});

describe('handlePartialFailure', () => {
  it('defaults to no partial / no notify', async () => {
    expect(await mk().handlePartialFailure(res(), err)).toEqual({ returnPartial: false, notify: false });
  });

  it('returns partial only for return_partial_with_flag', async () => {
    const a = mk({ partial_failure: { action: 'return_partial_with_flag', notify: true } });
    expect(await a.handlePartialFailure(res(), err)).toEqual({ returnPartial: true, notify: true });
    const b = mk({ partial_failure: { action: 'abort' } });
    expect(await b.handlePartialFailure(res(), err)).toEqual({ returnPartial: false, notify: false });
  });
});

describe('createErrorInfo', () => {
  it('maps error fields with default and custom code', () => {
    const e = new TypeError('bad');
    const info = mk().createErrorInfo(e);
    expect(info.code).toBe('UNKNOWN_ERROR');
    expect(info.message).toBe('bad');
    expect(info.details.name).toBe('TypeError');
    expect(mk().createErrorInfo(e, 'E1').code).toBe('E1');
  });
});

describe('shouldEscalate', () => {
  it('low confidence triggers escalation', () => {
    expect(mk().shouldEscalate(res({ confidence: 0.2 }), ['confidence < 0.5'])).toBe(true);
    expect(mk().shouldEscalate(res({ confidence: 0.9 }), ['confidence < 0.5'])).toBe(false);
  });

  it('critical_error triggers only when result has error', () => {
    const error = { code: 'E', message: 'm' };
    expect(mk().shouldEscalate(res({ error }), ['critical_error'])).toBe(true);
    expect(mk().shouldEscalate(res(), ['critical_error'])).toBe(false);
  });

  it('requires_approval short-circuits to false; empty/unknown conditions are false', () => {
    expect(mk().shouldEscalate(res({ confidence: 0.1 }), ['requires_approval', 'confidence < 0.5'])).toBe(false);
    expect(mk().shouldEscalate(res(), [])).toBe(false);
    expect(mk().shouldEscalate(res(), ['unknown'])).toBe(false);
  });
});
