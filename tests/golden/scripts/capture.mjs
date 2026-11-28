// Capture TS MCP server outputs as golden files.
// usage: node capture.mjs <distMcpDir> <outDir> <workDir>
import { spawn } from 'node:child_process';
import { writeFileSync, mkdirSync } from 'node:fs';
import { join } from 'node:path';

const [distDir, outDir, workDir] = process.argv.slice(2);
mkdirSync(outDir, { recursive: true });

const cases = {
  research: [
    { name: "execute", arguments: { task_id: "g-1", action: "research", parameters: { query: "CI/CD", depth: "detailed" } } },
    { name: "web_search", arguments: { query: "rust" } },
    { name: "web_fetch", arguments: { url: "https://example.com/x" } },
    { name: "document_parser", arguments: { content: "abc" } },
    { name: "nope", arguments: { a: 1 } },
    { name: "execute", arguments: { task_id: "g-1b", parameters: { query: "x" } } },
    { name: "execute", arguments: { task_id: 7, action: "research", parameters: {} } },
    { name: "execute", arguments: { task_id: "", action: "research" } },
    { name: "execute", arguments: { action: "research", parameters: [1] } },
  ],
  codegen: [
    { name: "execute", arguments: { task_id: "g-2", action: "codegen", parameters: { specification: "Make a util" } } },
    { name: "file_create", arguments: { path: "s.txt", content: "a.b axb a.b $1" } },
    { name: "str_replace", arguments: { file: "s.txt", search: "a.b", replace: "[$&]" } },
    { name: "str_replace", arguments: { file: "s.txt", search: "zzz", replace: "q" } },
    { name: "str_replace", arguments: { file: "s.txt", search: "", replace: "q" } },
    { name: "bash_tool", arguments: { command: "type s.txt" } },
  ],
  review: [
    { name: "execute", arguments: { task_id: "g-3", action: "review", parameters: { source_code: "fn a() {}\n// c\n" } } },
    { name: "bash_tool", arguments: { command: "echo src/a.ts(1,2): error TS2322: bad type" } },
    { name: "bash_tool", arguments: { command: "echo a.py:3:1: W291 trailing whitespace" } },
    { name: "bash_tool", arguments: { command: "echo plain warning line" } },
  ],
  document: [
    { name: "execute", arguments: { task_id: "g-4", action: "document", parameters: { content: "# T", format: "html" } } },
    { name: "notion_page_create", arguments: { parent_page_id: "p", title: "t", content: "x" } },
    { name: "notion_page_update", arguments: { title: "t" } },
    { name: "notion_page_update", arguments: { page_id: "p" } },
  ],
};

async function run(agent, calls) {
  const p = spawn(process.execPath, [join(distDir, `${agent}-server.js`)], { cwd: workDir, env: Object.fromEntries(Object.entries(process.env).filter(([k])=>!["NOTION_TOKEN","AGENT_ID","MAX_CONCURRENT"].includes(k))), stdio: ['pipe', 'pipe', 'pipe'] });
  let buf = '';
  const pending = new Map();
  const nonJson = [];
  p.stdout.on('data', (d) => {
    buf += d.toString();
    let i;
    while ((i = buf.indexOf('\n')) >= 0) {
      const line = buf.slice(0, i).trim();
      buf = buf.slice(i + 1);
      if (!line) continue;
      try {
        const msg = JSON.parse(line);
        if (msg.id !== undefined && pending.has(msg.id)) { pending.get(msg.id)(msg); pending.delete(msg.id); }
      } catch { nonJson.push(line); }
    }
  });
  let id = 0;
  const req = (method, params) => new Promise((res) => {
    const myId = ++id;
    pending.set(myId, res);
    p.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: myId, method, params }) + '\n');
  });
  const init = await req('initialize', { protocolVersion: '2024-11-05', capabilities: {}, clientInfo: { name: 'golden', version: '0' } });
  p.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n');
  const list = await req('tools/list', {});
  const results = [];
  for (const c of calls) results.push({ call: c, response: (await req('tools/call', c)).result ?? null });
  p.kill();
  writeFileSync(join(outDir, `${agent}.json`), JSON.stringify({ serverInfo: init.result.serverInfo, tools: list.result.tools, calls: results, stdout_non_json_lines: nonJson.length }, null, 2) + '\n');
}

for (const [agent, calls] of Object.entries(cases)) await run(agent, calls);
console.log('ok');
