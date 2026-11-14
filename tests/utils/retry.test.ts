import { describe, it, expect, vi, afterEach } from 'vitest';
import { RetryManager } from '../../src/utils/retry';
import { Logger } from '../../src/utils/logger';
import type { RetryPolicy } from '../../src/types/index';

const mk = (p: Partial<RetryPolicy> = {}) =>
  new RetryManager({ max_attempts: 3, backoff: 'fixed', initial_delay_ms: 100, ...p }, new Logger('t'));

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe('RetryManager backoff', () => {
  it('exponential doubles each attempt', () => {
    const r = mk({ backoff: 'exponential', max_attempts: 10 });
    expect([1, 2, 3, 4].map((a) => r.getNextDelay(a))).toEqual([100, 200, 400, 800]);
  });

  it('linear grows by base delay', () => {
    const r = mk({ backoff: 'linear', max_attempts: 10 });
    expect([1, 2, 3].map((a) => r.getNextDelay(a))).toEqual([100, 200, 300]);
  });

  it('fixed stays constant', () => {
    const r = mk({ max_attempts: 10 });
    expect([1, 2, 3].map((a) => r.getNextDelay(a))).toEqual([100, 100, 100]);
  });

  it('defaults base delay to 1000ms', () => {
    expect(mk({ initial_delay_ms: undefined }).getNextDelay(1)).toBe(1000);
  });

  it('returns 0 once attempts are exhausted', () => {
    expect(mk().getNextDelay(3)).toBe(0);
  });
});

describe('RetryManager.canRetry', () => {
  it('is true below max_attempts only', () => {
    const r = mk();
    expect(r.canRetry(2)).toBe(true);
    expect(r.canRetry(3)).toBe(false);
  });
});

describe('RetryManager.executeWithRetry', () => {
  it('returns immediately on success without calling onRetry', async () => {
    const fn = vi.fn().mockResolvedValue('ok');
    const onRetry = vi.fn();
    await expect(mk().executeWithRetry(fn, onRetry)).resolves.toBe('ok');
    expect(fn).toHaveBeenCalledTimes(1);
    expect(onRetry).not.toHaveBeenCalled();
  });

  it('retries after failure with backoff delays then succeeds', async () => {
    vi.useFakeTimers();
    const fn = vi
      .fn()
      .mockRejectedValueOnce(new Error('a'))
      .mockRejectedValueOnce(new Error('b'))
      .mockResolvedValue('done');
    const onRetry = vi.fn();
    const p = mk({ backoff: 'exponential' }).executeWithRetry(fn, onRetry);

    await vi.advanceTimersByTimeAsync(99);
    expect(fn).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(fn).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(199);
    expect(fn).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(1);

    await expect(p).resolves.toBe('done');
    expect(fn).toHaveBeenCalledTimes(3);
    expect(onRetry.mock.calls.map((c) => c[0])).toEqual([1, 2]);
  });

  it('throws the last error after max attempts', async () => {
    vi.useFakeTimers();
    const fn = vi.fn().mockRejectedValue(new Error('boom'));
    const assertion = expect(mk().executeWithRetry(fn)).rejects.toThrow('boom');
    await vi.runAllTimersAsync();
    await assertion;
    expect(fn).toHaveBeenCalledTimes(3);
  });

  it('wraps non-Error rejections', async () => {
    const fn = vi.fn().mockRejectedValue('str');
    await expect(mk({ max_attempts: 1 }).executeWithRetry(fn)).rejects.toThrow('str');
  });
});
