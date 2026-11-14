import { describe, it, expect } from 'vitest';
import { A2AProtocol } from './src/protocol/a2a';

const req = () =>
  A2AProtocol.createTaskRequest('a', 'b', { task_id: 't1', action: 'research', parameters: {} });

describe('A2AProtocol validation', () => {
  it('accepts every message created by the factories', () => {
    const msgs = [
      req(),
      A2AProtocol.createTaskResult('b', 'a', 'm1', { task_id: 't1', result: 1 }, 'completed'),
      A2AProtocol.createProgressMessage('b', 'a', 't1', 0.5, 'half'),
      A2AProtocol.createErrorMessage('b', 'a', 'm1', { code: 'X', message: 'boom' }),
      A2AProtocol.createHeartbeat('a', 'b'),
    ];
    for (const m of msgs) {
      expect(A2AProtocol.validateMessage(m)).toEqual({ valid: true });
      expect(A2AProtocol.isMessage(m)).toBe(true);
    }
  });

  it('rejects non-objects and missing envelope fields', () => {
    expect(A2AProtocol.validateMessage(null).valid).toBe(false);
    expect(A2AProtocol.validateMessage('x').valid).toBe(false);
    expect(A2AProtocol.validateMessage([]).valid).toBe(false);
    expect(A2AProtocol.validateMessage({ ...req(), from: undefined }).error).toBe('Missing from');
    expect(A2AProtocol.validateMessage({ ...req(), message_id: 5 }).valid).toBe(false);
    expect(A2AProtocol.validateMessage({ ...req(), timestamp: 'nope' }).valid).toBe(false);
  });

  it('rejects unknown type', () => {
    expect(A2AProtocol.validateMessage({ ...req(), type: 'bogus' }).valid).toBe(false);
  });

  it('rejects bad task_request payloads', () => {
    expect(A2AProtocol.validateMessage({ ...req(), payload: undefined }).valid).toBe(false);
    expect(A2AProtocol.validateMessage({ ...req(), payload: { task_id: 't', action: 'a' } }).valid).toBe(false);
    expect(
      A2AProtocol.validateMessage({ ...req(), payload: { task_id: 't', action: 'a', parameters: [] } }).valid
    ).toBe(false);
  });

  it('rejects task_result / error without in_reply_to, bad status, bad payload', () => {
    const res = A2AProtocol.createTaskResult('b', 'a', 'm1', { task_id: 't1' }, 'completed');
    expect(A2AProtocol.validateMessage({ ...res, in_reply_to: '' }).valid).toBe(false);
    expect(A2AProtocol.validateMessage({ ...res, status: 'weird' }).valid).toBe(false);
    expect(A2AProtocol.validateMessage({ ...res, payload: {} }).valid).toBe(false);
    const err = A2AProtocol.createErrorMessage('b', 'a', 'm1', { code: 'X', message: 'm' });
    expect(A2AProtocol.validateMessage({ ...err, payload: { error: { code: 1 } } }).valid).toBe(false);
  });

  it('rejects bad progress and heartbeat payloads', () => {
    const p = A2AProtocol.createProgressMessage('b', 'a', 't1', 1, 'm');
    expect(A2AProtocol.validateMessage({ ...p, payload: { task_id: 't1', progress: 'x', message: 'm' } }).valid).toBe(false);
    expect(A2AProtocol.validateMessage({ ...p, payload: { task_id: 't1', progress: NaN, message: 'm' } }).valid).toBe(false);
    const h = A2AProtocol.createHeartbeat('a', 'b');
    expect(A2AProtocol.validateMessage({ ...h, payload: { status: 'dead' } }).valid).toBe(false);
  });
});

describe('A2AProtocol serialize / deserialize', () => {
  it('round-trips a valid message', () => {
    const m = req();
    expect(A2AProtocol.deserialize(A2AProtocol.serialize(m))).toEqual(m);
  });

  it('throws on invalid message / malformed JSON / schema violation', () => {
    expect(() => A2AProtocol.serialize({ ...req(), type: 'bogus' } as any)).toThrow(/Invalid A2A message/);
    expect(() => A2AProtocol.deserialize('{not json')).toThrow(/malformed JSON/);
    expect(() => A2AProtocol.deserialize('{"type":"heartbeat"}')).toThrow(/Invalid A2A message/);
  });
});
