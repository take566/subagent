import { describe, it, expect } from 'vitest';
import { A2AProtocol } from '../../src/protocol/a2a';

const req = () =>
  A2AProtocol.createTaskRequest('a', 'b', { task_id: 't1', action: 'run', parameters: {} });

describe('A2AProtocol factories', () => {
  it('createTaskRequest defaults priority to medium and honors overrides', () => {
    expect(req().metadata?.priority).toBe('medium');
    const m = A2AProtocol.createTaskRequest(
      'a', 'b', { task_id: 't', action: 'x', parameters: {} },
      { priority: 'high', correlation_id: 'c1' }
    );
    expect(m.metadata).toEqual({ priority: 'high', correlation_id: 'c1' });
  });

  it('createTaskResult / createErrorMessage set reply linkage and status', () => {
    const r = A2AProtocol.createTaskResult('b', 'a', 'm1', { task_id: 't' }, 'completed');
    expect(r.in_reply_to).toBe('m1');
    expect(r.status).toBe('completed');
    const e = A2AProtocol.createErrorMessage('b', 'a', 'm1', { code: 'E', message: 'x' });
    expect(e.status).toBe('failed');
    expect(e.type).toBe('error');
  });

  it('generates unique message ids', () => {
    expect(req().message_id).not.toBe(req().message_id);
  });
});

describe('A2AProtocol.validateMessage', () => {
  it('accepts a valid message', () => {
    expect(A2AProtocol.validateMessage(req())).toEqual({ valid: true });
  });

  it('rejects non-object input', () => {
    for (const v of [null, undefined, 1, 'x', []]) {
      expect(A2AProtocol.validateMessage(v)).toEqual({ valid: false, error: 'Message must be an object' });
    }
  });

  it.each(['message_id', 'timestamp', 'from', 'to'])('reports missing / non-string %s', (key) => {
    expect(A2AProtocol.validateMessage({ ...req(), [key]: undefined }).error).toBe(`Missing ${key}`);
    expect(A2AProtocol.validateMessage({ ...req(), [key]: 1 }).error).toBe(`${key} must be a non-empty string`);
  });

  it('rejects invalid timestamp, missing/unknown type', () => {
    expect(A2AProtocol.validateMessage({ ...req(), timestamp: 'nope' }).valid).toBe(false);
    expect(A2AProtocol.validateMessage({ ...req(), type: undefined }).error).toBe('Missing type');
    expect(A2AProtocol.validateMessage({ ...req(), type: 'zzz' }).error).toMatch(/Unknown message type/);
  });

  it('requires in_reply_to and valid status on responses', () => {
    const r = A2AProtocol.createTaskResult('b', 'a', 'm1', { task_id: 't' }, 'completed');
    expect(A2AProtocol.validateMessage({ ...r, in_reply_to: undefined }).error).toMatch(/in_reply_to/);
    expect(A2AProtocol.validateMessage({ ...r, status: 'bad' }).error).toBe('Invalid status');
  });

  it('rejects non-object metadata', () => {
    expect(A2AProtocol.validateMessage({ ...req(), metadata: 'x' }).error).toBe('metadata must be an object');
  });

  it('validates payload per type', () => {
    const bad = (m: object, payload: unknown) => A2AProtocol.validateMessage({ ...m, payload }).valid;
    expect(bad(req(), { task_id: 't', action: 'a', parameters: {}, context: 1 })).toBe(false);
    expect(bad(req(), { task_id: '', action: 'a', parameters: {} })).toBe(false);
    expect(bad(req(), 'str')).toBe(false);
    const res = A2AProtocol.createTaskResult('b', 'a', 'm', { task_id: 't' }, 'completed');
    expect(bad(res, { task_id: 't', error: { code: 1 } })).toBe(false);
    const p = A2AProtocol.createProgressMessage('b', 'a', 't', 0.1, 'm');
    expect(bad(p, { task_id: 't', progress: Infinity, message: 'm' })).toBe(false);
    expect(bad(p, { task_id: 't', progress: 1, message: 2 })).toBe(false);
    const h = A2AProtocol.createHeartbeat('a', 'b');
    expect(bad(h, { status: 'dead' })).toBe(false);
  });
});

describe('A2AProtocol serialize / deserialize / isMessage', () => {
  it('round-trips', () => {
    const m = req();
    expect(A2AProtocol.deserialize(A2AProtocol.serialize(m))).toEqual(m);
  });

  it('throws on invalid input', () => {
    expect(() => A2AProtocol.serialize({ ...req(), type: 'x' } as any)).toThrow(/Invalid A2A message/);
    expect(() => A2AProtocol.deserialize('{oops')).toThrow(/malformed JSON/);
    expect(() => A2AProtocol.deserialize('{}')).toThrow(/Invalid A2A message/);
  });

  it('isMessage mirrors validation', () => {
    expect(A2AProtocol.isMessage(req())).toBe(true);
    expect(A2AProtocol.isMessage({})).toBe(false);
  });
});
