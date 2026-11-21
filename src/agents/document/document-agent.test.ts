import { describe, it, expect, beforeAll, afterAll } from 'vitest';
import * as fs from 'fs/promises';
import * as os from 'os';
import * as path from 'path';
import { DocumentAgent } from './document-agent.js';

describe('DocumentAgent docx/pdf output', () => {
  const origCwd = process.cwd();
  let tmp: string;

  beforeAll(async () => {
    tmp = await fs.mkdtemp(path.join(os.tmpdir(), 'docagent-'));
    process.chdir(tmp);
  });

  afterAll(async () => {
    process.chdir(origCwd);
    await fs.rm(tmp, { recursive: true, force: true });
  });

  const run = async (format: string) => {
    const agent = new DocumentAgent();
    const res = await agent.execute({
      id: `t-${format}`,
      type: 'document',
      parameters: { content: '# Title\n\nbody text', format },
    } as any);
    return fs.readFile((res.result as any).document_path);
  };

  it('format=docx は PK で始まる zip を生成する', async () => {
    const buf = await run('docx');
    expect(buf.subarray(0, 2).toString('latin1')).toBe('PK');
  });

  it('format=pdf は %PDF で始まるファイルを生成する', async () => {
    const buf = await run('pdf');
    expect(buf.subarray(0, 4).toString('latin1')).toBe('%PDF');
  });
});
