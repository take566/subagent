import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { NotionPageCreateTool, NotionPageUpdateTool } from './src/agents/document/document-agent';

const okResponse = (body: unknown) => ({ ok: true, json: async () => body });

describe('Notion tools', () => {
  const fetchMock = vi.fn();
  const original = process.env.NOTION_TOKEN;

  beforeEach(() => {
    fetchMock.mockReset();
    fetchMock.mockResolvedValue(okResponse({ id: 'pid', url: 'u' }));
    vi.stubGlobal('fetch', fetchMock);
    process.env.NOTION_TOKEN = 'test-token';
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    if (original === undefined) delete process.env.NOTION_TOKEN;
    else process.env.NOTION_TOKEN = original;
  });

  it('errors without NOTION_TOKEN and does not call fetch', async () => {
    delete process.env.NOTION_TOKEN;
    await expect(
      new NotionPageCreateTool().invoke({ parent_page_id: 'p', title: 't' })
    ).rejects.toThrow(/NOTION_TOKEN/);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('rejects empty title / parent / content', async () => {
    await expect(
      new NotionPageCreateTool().invoke({ parent_page_id: 'p', title: '' })
    ).rejects.toThrow();
    await expect(new NotionPageUpdateTool().invoke({ page_id: 'p' })).rejects.toThrow();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('create appends blocks beyond 100 instead of dropping them', async () => {
    const content = Array.from({ length: 250 }, (_, i) => `line ${i}`).join('\n');
    await new NotionPageCreateTool().invoke({ parent_page_id: 'p', title: 't', content });
    expect(fetchMock).toHaveBeenCalledTimes(3);
    const sizes = fetchMock.mock.calls.map((c) => {
      const b = JSON.parse(c[1].body);
      return b.children.length;
    });
    expect(sizes).toEqual([100, 100, 50]);
    expect(fetchMock.mock.calls[1][0]).toContain('/blocks/pid/children');
  });

  it('splits lines longer than 2000 chars', async () => {
    await new NotionPageCreateTool().invoke({
      parent_page_id: 'p',
      title: 't',
      content: 'a'.repeat(4500),
    });
    const children = JSON.parse(fetchMock.mock.calls[0][1].body).children;
    expect(children.map((b: any) => b.paragraph.rich_text[0].text.content.length)).toEqual([
      2000, 2000, 500,
    ]);
  });
});
