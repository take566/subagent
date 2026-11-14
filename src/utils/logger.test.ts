import { describe, it, expect, vi, afterEach } from 'vitest';
import { Logger } from './logger.js';

describe('Logger structured fields', () => {
  afterEach(() => vi.restoreAllMocks());

  it('child() のログに agent_id / task_id / correlation_id / duration / status が載る', () => {
    const spy = vi.spyOn(console, 'info').mockImplementation(() => {});
    new Logger('agent-1')
      .child({ task_id: 't1', correlation_id: 'c1' })
      .info('done', undefined, { status: 'success', duration: 12 });

    const entry = JSON.parse(spy.mock.calls[0][0] as string);
    expect(entry).toMatchObject({
      agent_id: 'agent-1',
      task_id: 't1',
      correlation_id: 'c1',
      status: 'success',
      duration: 12,
    });
  });

  it('親ロガーには task_id が漏れない', () => {
    const spy = vi.spyOn(console, 'info').mockImplementation(() => {});
    const parent = new Logger('a');
    parent.child({ task_id: 't1' });
    parent.info('x');
    expect(JSON.parse(spy.mock.calls[0][0] as string).task_id).toBeUndefined();
  });
});
