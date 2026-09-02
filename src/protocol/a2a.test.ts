import { describe, it, expect } from 'vitest';
import { A2AProtocol } from './a2a.js';
import { isA2AMessage, parseA2AMessage } from './a2a-schema.js';

describe('A2AProtocol.validateMessage', () => {
  it('accepts a valid task_request', () => {
    const msg = A2AProtocol.createTaskRequest('orch', 'research-001', {
      task_id: 't1',
      action: 'research',
      parameters: { query: 'x' },
    });
    expect(A2AProtocol.validateMessage(msg).valid).toBe(true);
  });

  it('rejects missing payload fields', () => {
    const result = A2AProtocol.validateMessage({
      message_id: 'm1',
      timestamp: new Date().toISOString(),
      from: 'a',
      to: 'b',
      type: 'task_request',
      payload: { task_id: 't1' },
    });
    expect(result.valid).toBe(false);
    expect(result.error).toMatch(/action/);
  });

  it('requires in_reply_to for task_result', () => {
    const result = A2AProtocol.validateMessage({
      message_id: 'm1',
      timestamp: new Date().toISOString(),
      from: 'a',
      to: 'b',
      type: 'task_result',
      payload: { task_id: 't1' },
    });
    expect(result.valid).toBe(false);
  });

  it('serialize/deserialize roundtrip', () => {
    const msg = A2AProtocol.createProgressMessage(
      'a',
      'b',
      't1',
      0.5,
      'halfway'
    );
    const raw = A2AProtocol.serialize(msg);
    const back = A2AProtocol.deserialize(raw);
    expect(back.type).toBe('progress');
    expect(isA2AMessage(back)).toBe(true);
  });

  it('deserialize throws on invalid payload', () => {
    expect(() =>
      A2AProtocol.deserialize(
        JSON.stringify({
          message_id: 'm',
          timestamp: 't',
          from: 'a',
          to: 'b',
          type: 'progress',
          payload: { task_id: 't', progress: 2, message: 'bad' },
        })
      )
    ).toThrow(/Invalid A2A message/);
  });

  it('parseA2AMessage reports structured failure', () => {
    const parsed = parseA2AMessage({ type: 'heartbeat' });
    expect(parsed.success).toBe(false);
  });
});
