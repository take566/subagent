import { describe, it, expect } from 'vitest';
import { TaskParser } from './task-parser.js';
import type { Task } from '../types/index.js';

describe('TaskParser', () => {
  const parser = new TaskParser();

  it('treats a plain string as an execute task with the query', () => {
    const [t] = parser.parse('hello world');
    expect(t.action).toBe('execute');
    expect(t.parameters).toEqual({ query: 'hello world' });
    expect(t.id).toMatch(/^task-/);
  });

  it('parses a JSON object string', () => {
    const [t] = parser.parse(JSON.stringify({ id: 'a', action: 'research', parameters: { q: 1 } }));
    expect(t).toMatchObject({ id: 'a', action: 'research', parameters: { q: 1 } });
  });

  it('parses a JSON array string', () => {
    const tasks = parser.parse(JSON.stringify([{ id: 'a' }, { id: 'b' }]));
    expect(tasks.map((t) => t.id)).toEqual(['a', 'b']);
  });

  it('normalizes a single task and fills defaults', () => {
    const [t] = parser.parse({ id: 'x' } as Task);
    expect(t.action).toBe('execute');
    expect(t.parameters).toEqual({});
    expect(t.metadata?.created_at).toBeTypeOf('string');
  });

  it('keeps an existing created_at and normalizes arrays', () => {
    const tasks = parser.parse([
      { id: 'a', action: 'review', parameters: {}, metadata: { created_at: '2020-01-01T00:00:00.000Z' } },
      { id: 'b', action: 'review', parameters: {} },
    ]);
    expect(tasks).toHaveLength(2);
    expect(tasks[0].metadata?.created_at).toBe('2020-01-01T00:00:00.000Z');
    expect(tasks[1].metadata?.created_at).toBeTypeOf('string');
  });

  it('falls back to a query task for malformed JSON', () => {
    const [t] = parser.parse('{not json');
    expect(t.parameters).toEqual({ query: '{not json' });
  });

  it('generates distinct ids', () => {
    const [a] = parser.parse('x');
    const [b] = parser.parse('x');
    expect(a.id).not.toBe(b.id);
  });

  describe('extractDependencies', () => {
    it('extracts parent_task and correlation_id dependencies', () => {
      const tasks: Task[] = [
        { id: 'p', action: 'a', parameters: {} },
        { id: 'c', action: 'a', parameters: {}, context: { parent_task: 'p' } },
        { id: 'x', action: 'a', parameters: {}, metadata: { correlation_id: 'k' } },
        { id: 'y', action: 'a', parameters: {}, metadata: { correlation_id: 'k' } },
      ];
      const deps = parser.extractDependencies(tasks);
      expect(deps.has('p')).toBe(false);
      expect(deps.get('c')).toEqual(['p']);
      expect(deps.get('x')).toEqual(['y']);
      expect(deps.get('y')).toEqual(['x']);
    });

    it('returns an empty map when there are no dependencies', () => {
      expect(parser.extractDependencies([{ id: 'a', action: 'a', parameters: {} }]).size).toBe(0);
    });
  });
});
