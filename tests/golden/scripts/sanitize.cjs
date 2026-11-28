// usage: node sanitize.cjs <inDir> <outDir> <workDirAbs>
const fs = require('fs');
const path = require('path');
const [inDir, outDir, work] = process.argv.slice(2);
const workWin = work.split('/').join('\\');
const fix = (v) =>
  typeof v === 'string'
    ? v.split(workWin).join('<WORK>').split(work).join('<WORK>').split('\\').join('/')
        .replace(/generated-\d{4}-\d\d-\d\dT\d\d-\d\d-\d\d-\d{3}Z/, 'generated-<TIMESTAMP>')
    : Array.isArray(v)
      ? v.map(fix)
      : v && typeof v === 'object'
        ? Object.fromEntries(Object.entries(v).map(([k, x]) => [k, k === 'duration_ms' ? '<DURATION>' : fix(x)]))
        : v;
// シェルの出力に依存するケースは OS ごとに変わるので golden から外す
const shellDependent = (c) => c.call.name === 'bash_tool';
for (const a of ['research', 'codegen', 'review', 'document']) {
  const g = JSON.parse(fs.readFileSync(path.join(inDir, a + '.json'), 'utf8'));
  g.calls = g.calls.filter((c) => !shellDependent(c));
  for (const c of g.calls) {
    const ct = c.response.content[0];
    let o;
    try { o = JSON.parse(ct.text); } catch { continue; }
    o = fix(o);
    if (a === 'review' && o.result && Array.isArray(o.result.issues)) {
      o.result.issues = '<TS ran `npm run lint`; npm error lines were counted as lint errors. Rust: lint only when lint_command is given>';
    }
    ct.text = JSON.stringify(o, null, 2);
  }
  fs.writeFileSync(path.join(outDir, a + '.json'), JSON.stringify(g, null, 2) + '\n');
}
