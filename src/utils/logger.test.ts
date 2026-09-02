import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { Logger, LogLevel } from './logger.js';

describe('Logger structured fields', () => {
  let spy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    spy = vi.spyOn(console, 'info').mockImplementation(() => {});
  });

  afterEach(() => {
    spy.mockRestore();
  });

  it('includes task_id and correlation_id from context', () => {
    const logger = new Logger('orch', LogLevel.INFO);
    logger.setContext({ task_id: 't1', correlation_id: 'c1' });
    logger.info('hello');
    const entry = JSON.parse(String(spy.mock.calls[0][0]));
    expect(entry.agent_id).toBe('orch');
    expect(entry.task_id).toBe('t1');
    expect(entry.correlation_id).toBe('c1');
    expect(entry.message).toBe('hello');
  });

  it('allows per-call overrides', () => {
    const logger = new Logger('orch');
    logger.setContext({ task_id: 't1' });
    logger.info('done', { task_id: 't2', status: 'completed', duration_ms: 10 });
    const entry = JSON.parse(String(spy.mock.calls[0][0]));
    expect(entry.task_id).toBe('t2');
    expect(entry.status).toBe('completed');
    expect(entry.duration_ms).toBe(10);
  });
});
