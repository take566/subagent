import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import * as fs from 'fs/promises';
import * as os from 'os';
import * as path from 'path';
import { CodeGenAgent, StrReplaceTool } from '../../src/agents/codegen/codegen-agent';
import type { Task } from '../../src/types/index';

const task = { id: 't2', type: 'codegen', parameters: { specification: 'hello' } } as unknown as Task;

describe('CodeGenAgent (mock tools)', () => {
  it('happy path: file_create を本体とテスト分だけ呼び completed', async () => {
    const agent = new CodeGenAgent();
    const create = vi
      .spyOn(agent.getTool('file_create')!, 'invoke')
      .mockImplementation(async (p) => ({ path: p.path, created: true }));

    const res = await agent.execute(task);

    expect(res.status).toBe('completed');
    expect(create).toHaveBeenCalledTimes(2);
    expect(res.result.files).toHaveLength(1);
    expect(res.result.test_files).toHaveLength(1);
    expect(agent.getState().status).toBe('completed');
  });

  it('failure: file_create が例外を投げると failed を記録して throw する', async () => {
    const agent = new CodeGenAgent();
    vi.spyOn(agent.getTool('file_create')!, 'invoke').mockRejectedValue(new Error('disk full'));

    await expect(agent.execute(task)).rejects.toThrow('disk full');

    const state = agent.getState();
    expect(state.status).toBe('failed');
    expect(state.context.history[0].status).toBe('failed');
    expect(state.context.history[0].error?.code).toBe('CODEGEN_ERROR');
  });
});

describe('StrReplaceTool (literal replace)', () => {
  let dir: string;
  let file: string;
  const tool = new StrReplaceTool();

  beforeEach(async () => {
    dir = await fs.mkdtemp(path.join(os.tmpdir(), 'str-replace-'));
    file = path.join(dir, 'target.txt');
  });

  afterEach(async () => {
    await fs.rm(dir, { recursive: true, force: true });
  });

  const run = async (content: string, search: string, replace: string) => {
    await fs.writeFile(file, content, 'utf-8');
    const res = await tool.invoke({ file, search, replace });
    return { res, after: await fs.readFile(file, 'utf-8') };
  };

  it("search='a.b' は 'axb' にマッチせず 'a.b' だけを置換する", async () => {
    const { res, after } = await run('a.b axb a.b', 'a.b', 'Z');
    expect(after).toBe('Z axb Z');
    expect(res).toEqual({ file, replaced: true, occurrences: 2 });
  });

  it("search='foo(' や '[x]' でも例外を出さずリテラル一致で置換する", async () => {
    const r1 = await run('call foo(1); foo(2)', 'foo(', 'bar(');
    expect(r1.after).toBe('call bar(1); bar(2)');
    expect(r1.res.occurrences).toBe(2);

    const r2 = await run('arr[x] = x', '[x]', '[y]');
    expect(r2.after).toBe('arr[y] = x');
    expect(r2.res.occurrences).toBe(1);
  });

  it("replace='$1' や '$&' はそのままの文字列として書き込む", async () => {
    const r1 = await run('price', 'price', '$1');
    expect(r1.after).toBe('$1');

    const r2 = await run('price', 'price', '$&$&');
    expect(r2.after).toBe('$&$&');
  });

  it('一致 0 件なら replaced=false, occurrences=0 でファイルを書き戻さない', async () => {
    await fs.writeFile(file, 'unchanged', 'utf-8');
    const old = new Date('2000-01-01T00:00:00Z');
    await fs.utimes(file, old, old);

    const res = await tool.invoke({ file, search: 'nomatch', replace: 'X' });

    expect(res).toEqual({ file, replaced: false, occurrences: 0 });
    expect(await fs.readFile(file, 'utf-8')).toBe('unchanged');
    expect((await fs.stat(file)).mtime.getTime()).toBe(old.getTime());
  });
});
