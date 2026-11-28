//! 組み込みツール実装（複数のエージェントで共有）
//!
//! ファイルを扱うツール（`file_create` / `str_replace` / `view`）は [`Workspace`] を、
//! コマンドを実行するツール（`bash_tool`）は [`Sandbox::run_line`] を通す。
//! `SUBAGENT_ROOTS` が無ければ拒否し、コマンドは `SUBAGENT_CODEGEN_ALLOWED_COMMANDS` にある
//! program だけを shell なしで実行する（[`crate::sandbox`]）。

pub mod lint_parsers;
pub mod notion;

pub use notion::{NotionClient, NotionPageCreateTool, NotionPageUpdateTool};

use crate::core::tool::{Tool, param_defs};
use crate::error::{Error, Result};
use crate::sandbox::{CommandOutput, Sandbox, Workspace};
use crate::types::{JsonMap, js_len};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;

/// 必須の文字列パラメータを取得
pub fn required_str<'a>(params: &'a JsonMap, key: &str) -> Result<&'a str> {
    params
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::InvalidParameter(format!("`{key}` (string) is required")))
}

/// 任意の文字列パラメータを取得
pub fn optional_str<'a>(params: &'a JsonMap, key: &str) -> Option<&'a str> {
    params.get(key).and_then(Value::as_str)
}

/// Workspace の（同期の）操作を blocking thread で実行する
async fn on_workspace<T, F>(sandbox: &Sandbox, f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(&Workspace) -> Result<T> + Send + 'static,
{
    let ws = sandbox.workspace()?;
    tokio::task::spawn_blocking(move || f(&ws))
        .await
        .map_err(|e| Error::other(format!("workspace task failed: {e}")))?
}

// ---------------------------------------------------------------------------
// file_create
// ---------------------------------------------------------------------------

/// ファイル作成ツール（workspace 内の相対 path だけ）
pub struct FileCreateTool {
    params: JsonMap,
    sandbox: Arc<Sandbox>,
}

impl FileCreateTool {
    pub fn new(sandbox: Arc<Sandbox>) -> Self {
        FileCreateTool {
            params: param_defs(&[("path", "string", true), ("content", "string", true)]),
            sandbox,
        }
    }
}

#[async_trait]
impl Tool for FileCreateTool {
    fn name(&self) -> &str {
        "file_create"
    }
    fn description(&self) -> &str {
        "ファイルを作成"
    }
    fn parameters(&self) -> &JsonMap {
        &self.params
    }

    async fn invoke(&self, params: JsonMap) -> Result<Value> {
        let path = required_str(&params, "path")?.to_string();
        let content = required_str(&params, "content")?.to_string();
        let size = js_len(&content);
        let shown = on_workspace(&self.sandbox, move |ws| {
            ws.create(&path, &content)?;
            ws.display_path(&path)
        })
        .await?;
        Ok(json!({
            "path": shown.to_string_lossy(),
            "created": true,
            "size": size,
        }))
    }
}

// ---------------------------------------------------------------------------
// bash_tool / lint
// ---------------------------------------------------------------------------

/// 実行結果の共通部分（`success` / `error` / `stdout` / `stderr`）
fn command_json(command: &str, out: &CommandOutput) -> JsonMap {
    let mut m = JsonMap::new();
    m.insert("success".into(), json!(out.success));
    if !out.success {
        let error = if out.timed_out {
            format!("Command timed out: {command}")
        } else {
            format!("Command failed: {command}\n{}", out.stderr)
        };
        m.insert("error".into(), json!(error));
    }
    m.insert("stdout".into(), json!(out.stdout));
    m.insert("stderr".into(), json!(out.stderr));
    if out.timed_out {
        m.insert("timed_out".into(), json!(true));
    }
    if out.truncated {
        m.insert("truncated".into(), json!(true));
    }
    m
}

/// コマンド実行ツール。名前は互換のため `bash_tool` だが shell は使わない
/// （`command` は program と引数。`cwd` は workspace 内の相対 path）
pub struct BashTool {
    params: JsonMap,
    sandbox: Arc<Sandbox>,
}

impl BashTool {
    pub fn new(sandbox: Arc<Sandbox>) -> Self {
        BashTool {
            params: param_defs(&[("command", "string", true), ("cwd", "string", false)]),
            sandbox,
        }
    }
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash_tool"
    }
    fn description(&self) -> &str {
        "Bashコマンドを実行"
    }
    fn parameters(&self) -> &JsonMap {
        &self.params
    }

    async fn invoke(&self, params: JsonMap) -> Result<Value> {
        let command = required_str(&params, "command")?;
        let out = self
            .sandbox
            .run_line(command, optional_str(&params, "cwd"))
            .await?;
        Ok(Value::Object(command_json(command, &out)))
    }
}

/// Lint 実行ツール（ツール名は TS 版と同じく `bash_tool`。実行は [`BashTool`] と同じ制限）
pub struct LintTool {
    params: JsonMap,
    sandbox: Arc<Sandbox>,
}

impl LintTool {
    pub fn new(sandbox: Arc<Sandbox>) -> Self {
        LintTool {
            params: param_defs(&[("command", "string", true), ("cwd", "string", false)]),
            sandbox,
        }
    }
}

/// lint 出力を言語別に解釈して issues（JSON）にする。
/// ESLint / tsc / flake8・ruff のどれにも当たらなければ `error` / `warning` を含む行を拾う。
/// 詳細は [`lint_parsers::parse_lint_findings`]。
pub fn parse_lint_output(output: &str) -> Vec<Value> {
    lint_parsers::parse_lint_findings(output)
        .into_iter()
        .map(|f| serde_json::to_value(f).expect("LintFinding serializes"))
        .collect()
}

#[async_trait]
impl Tool for LintTool {
    fn name(&self) -> &str {
        "bash_tool"
    }
    fn description(&self) -> &str {
        "Lintを実行"
    }
    fn parameters(&self) -> &JsonMap {
        &self.params
    }

    async fn invoke(&self, params: JsonMap) -> Result<Value> {
        let command = required_str(&params, "command")?;
        let out = self
            .sandbox
            .run_line(command, optional_str(&params, "cwd"))
            .await?;
        // TS は stdout だけを見ていたが、cargo clippy のように指摘を stderr に出すツールも
        // あるので両方を解釈する（stdout → stderr の順。言語の推定はそれぞれ別に行う）
        let mut issues = parse_lint_output(&out.stdout);
        issues.extend(parse_lint_output(&out.stderr));
        let mut m = command_json(command, &out);
        m.insert("issues".into(), Value::Array(issues));
        Ok(Value::Object(m))
    }
}

// ---------------------------------------------------------------------------
// str_replace
// ---------------------------------------------------------------------------

/// 文字列置換ツール（workspace 内の相対 path だけ）。
///
/// `search` はリテラル文字列として扱い（正規表現ではない）、`replace` 内の `$1` `$&` なども
/// 置換パターンとして解釈せずそのまま書き込む（TS #53）。一致はすべて置き換える。
/// 一致が 0 件ならファイルを書き戻さず `replaced: false` を返す。
/// （「ちょうど 1 回」の置換は [`Workspace::str_replace`]）
pub struct StrReplaceTool {
    params: JsonMap,
    sandbox: Arc<Sandbox>,
}

impl StrReplaceTool {
    pub fn new(sandbox: Arc<Sandbox>) -> Self {
        StrReplaceTool {
            params: param_defs(&[
                ("file", "string", true),
                ("search", "string", true),
                ("replace", "string", true),
            ]),
            sandbox,
        }
    }
}

#[async_trait]
impl Tool for StrReplaceTool {
    fn name(&self) -> &str {
        "str_replace"
    }
    fn description(&self) -> &str {
        "文字列を置換"
    }
    fn parameters(&self) -> &JsonMap {
        &self.params
    }

    async fn invoke(&self, params: JsonMap) -> Result<Value> {
        let file = required_str(&params, "file")?.to_string();
        let search = required_str(&params, "search")?.to_string();
        let replace = required_str(&params, "replace")?.to_string();
        let f = file.clone();
        let occurrences = on_workspace(&self.sandbox, move |ws| {
            ws.replace_all(&f, &search, &replace)
        })
        .await?;
        Ok(json!({ "file": file, "replaced": occurrences > 0, "occurrences": occurrences }))
    }
}

// ---------------------------------------------------------------------------
// view
// ---------------------------------------------------------------------------

/// ファイル閲覧ツール（workspace 内の相対 path だけ）
pub struct ViewTool {
    params: JsonMap,
    sandbox: Arc<Sandbox>,
}

impl ViewTool {
    pub fn new(sandbox: Arc<Sandbox>) -> Self {
        ViewTool {
            params: param_defs(&[("path", "string", true)]),
            sandbox,
        }
    }
}

#[async_trait]
impl Tool for ViewTool {
    fn name(&self) -> &str {
        "view"
    }
    fn description(&self) -> &str {
        "ファイルの内容を閲覧"
    }
    fn parameters(&self) -> &JsonMap {
        &self.params
    }

    async fn invoke(&self, params: JsonMap) -> Result<Value> {
        let path = required_str(&params, "path")?.to_string();
        let p = path.clone();
        let content = on_workspace(&self.sandbox, move |ws| ws.read_text(&p)).await?;
        Ok(json!({
            "path": path,
            "size": js_len(&content),
            "lines": content.split('\n').count(),
            "content": content,
        }))
    }
}

// ---------------------------------------------------------------------------
// research 用モックツール
// ---------------------------------------------------------------------------

/// JS のテンプレート文字列での値の表示（未定義は `undefined`）
pub fn js_display(v: Option<&Value>) -> String {
    match v {
        None => "undefined".into(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) => "null".into(),
        Some(Value::Array(a)) => a
            .iter()
            .map(|x| match x {
                Value::Null => String::new(),
                other => js_display(Some(other)),
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".into(),
        Some(other) => other.to_string(),
    }
}

/// Web 検索ツール（モック実装）
pub struct WebSearchTool {
    params: JsonMap,
}

impl Default for WebSearchTool {
    fn default() -> Self {
        WebSearchTool {
            params: param_defs(&[("query", "string", true), ("max_results", "number", false)]),
        }
    }
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }
    fn description(&self) -> &str {
        "Web検索を実行"
    }
    fn parameters(&self) -> &JsonMap {
        &self.params
    }

    async fn invoke(&self, params: JsonMap) -> Result<Value> {
        // 実際の実装では web_search API を呼び出す
        let query = params.get("query");
        let q = js_display(query);
        let mut out = JsonMap::new();
        out.insert(
            "results".into(),
            json!([{
                "title": format!("検索結果: {q}"),
                "url": "https://example.com/result1",
                "snippet": format!("{q}に関する情報です。"),
            }]),
        );
        if let Some(query) = query {
            out.insert("query".into(), query.clone());
        }
        out.insert("count".into(), json!(1));
        Ok(Value::Object(out))
    }
}

/// Web 取得ツール（モック実装）
pub struct WebFetchTool {
    params: JsonMap,
}

impl Default for WebFetchTool {
    fn default() -> Self {
        WebFetchTool {
            params: param_defs(&[("url", "string", true)]),
        }
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }
    fn description(&self) -> &str {
        "Webページの内容を取得"
    }
    fn parameters(&self) -> &JsonMap {
        &self.params
    }

    async fn invoke(&self, params: JsonMap) -> Result<Value> {
        // 実際の実装では HTTP リクエストを実行
        let url = params.get("url");
        let mut out = JsonMap::new();
        if let Some(url) = url {
            out.insert("url".into(), url.clone());
        }
        out.insert(
            "content".into(),
            json!(format!("ページの内容: {}", js_display(url))),
        );
        out.insert("title".into(), json!("ページタイトル"));
        Ok(Value::Object(out))
    }
}

/// ドキュメント解析ツール（モック実装）
pub struct DocumentParserTool {
    params: JsonMap,
}

impl Default for DocumentParserTool {
    fn default() -> Self {
        DocumentParserTool {
            params: param_defs(&[("content", "string", true), ("format", "string", false)]),
        }
    }
}

#[async_trait]
impl Tool for DocumentParserTool {
    fn name(&self) -> &str {
        "document_parser"
    }
    fn description(&self) -> &str {
        "ドキュメントを解析"
    }
    fn parameters(&self) -> &JsonMap {
        &self.params
    }

    async fn invoke(&self, _params: JsonMap) -> Result<Value> {
        Ok(json!({
            "parsed": true,
            "sections": ["セクション1", "セクション2"],
            "summary": "ドキュメントの要約",
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::{CommandPolicy, Workspace};
    use std::path::Path;

    fn map(v: Value) -> JsonMap {
        v.as_object().unwrap().clone()
    }

    fn sandbox_in(dir: &Path, policy: CommandPolicy) -> Arc<Sandbox> {
        Arc::new(Sandbox::new(
            Workspace::new(vec![dir.to_path_buf()]).unwrap(),
            policy,
        ))
    }

    fn files_only(dir: &Path) -> Arc<Sandbox> {
        sandbox_in(dir, CommandPolicy::disabled())
    }

    /// roots 未設定の sandbox
    fn no_roots() -> Arc<Sandbox> {
        Arc::new(Sandbox::from_lookup(|_| None))
    }

    #[tokio::test]
    async fn file_create_and_str_replace_and_view() {
        let dir = tempfile::tempdir().unwrap();
        let sb = files_only(dir.path());
        let r = FileCreateTool::new(sb.clone())
            .invoke(map(
                json!({"path": "a/b.txt", "content": "foo foo bar 日本"}),
            ))
            .await
            .unwrap();
        assert_eq!(r["size"], 14); // UTF-16 単位（JS の length と同じ）
        assert_eq!(
            r["path"],
            json!(dir.path().join("a").join("b.txt").to_string_lossy())
        );
        let p = dir.path().join("a/b.txt");
        assert!(p.exists());

        // search はリテラル（TS #53）。`fo+` は正規表現として解釈されないので一致しない
        let r = StrReplaceTool::new(sb.clone())
            .invoke(map(
                json!({"file": "a/b.txt", "search": "fo+", "replace": "X"}),
            ))
            .await
            .unwrap();
        assert_eq!(
            r,
            json!({"file": "a/b.txt", "replaced": false, "occurrences": 0})
        );
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "foo foo bar 日本");

        let r = StrReplaceTool::new(sb.clone())
            .invoke(map(
                json!({"file": "a/b.txt", "search": "foo", "replace": "X"}),
            ))
            .await
            .unwrap();
        assert_eq!(
            r,
            json!({"file": "a/b.txt", "replaced": true, "occurrences": 2})
        );
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "X X bar 日本");

        let v = ViewTool::new(sb)
            .invoke(map(json!({"path": "a/b.txt"})))
            .await
            .unwrap();
        assert_eq!(v["lines"], 1);
        assert_eq!(v["content"], "X X bar 日本");
    }

    #[tokio::test]
    async fn file_tools_refuse_paths_outside_the_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("s.txt");
        std::fs::write(&secret, "secret").unwrap();
        let abs = secret.to_string_lossy().into_owned();
        let sb = files_only(dir.path());
        for e in [
            ViewTool::new(sb.clone())
                .invoke(map(json!({"path": abs})))
                .await
                .unwrap_err(),
            FileCreateTool::new(sb.clone())
                .invoke(map(json!({"path": "../s.txt", "content": "x"})))
                .await
                .unwrap_err(),
            StrReplaceTool::new(sb)
                .invoke(map(
                    json!({"file": abs, "search": "secret", "replace": "x"}),
                ))
                .await
                .unwrap_err(),
        ] {
            assert!(matches!(e, Error::Sandbox(_)), "{e}");
        }
        assert_eq!(std::fs::read_to_string(&secret).unwrap(), "secret");
    }

    #[tokio::test]
    async fn file_tools_refuse_without_roots() {
        for e in [
            FileCreateTool::new(no_roots())
                .invoke(map(json!({"path": "a.txt", "content": "x"})))
                .await
                .unwrap_err(),
            ViewTool::new(no_roots())
                .invoke(map(json!({"path": "a.txt"})))
                .await
                .unwrap_err(),
            BashTool::new(Arc::new(Sandbox::from_lookup(|k| {
                (k == "SUBAGENT_CODEGEN_ALLOWED_COMMANDS").then(|| "cargo".into())
            })))
            .invoke(map(json!({"command": "cargo --version"})))
            .await
            .unwrap_err(),
        ] {
            assert!(e.to_string().contains("SUBAGENT_ROOTS"), "{e}");
        }
    }

    // --- StrReplaceTool (literal replace)。TS tests/agents/codegen-agent.test.ts の移植 ---

    async fn str_replace_run(content: &str, search: &str, replace: &str) -> (Value, String) {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("target.txt");
        std::fs::write(&file, content).unwrap();
        let res = StrReplaceTool::new(files_only(dir.path()))
            .invoke(map(
                json!({"file": "target.txt", "search": search, "replace": replace}),
            ))
            .await
            .unwrap();
        let after = std::fs::read_to_string(&file).unwrap();
        // file は入力のまま返す
        assert_eq!(res["file"], json!("target.txt"));
        (res, after)
    }

    #[tokio::test]
    async fn str_replace_dot_is_literal() {
        let (res, after) = str_replace_run("a.b axb a.b", "a.b", "Z").await;
        assert_eq!(after, "Z axb Z");
        assert_eq!(res["replaced"], true);
        assert_eq!(res["occurrences"], 2);
    }

    #[tokio::test]
    async fn str_replace_regex_metachars_do_not_error() {
        let (res, after) = str_replace_run("call foo(1); foo(2)", "foo(", "bar(").await;
        assert_eq!(after, "call bar(1); bar(2)");
        assert_eq!(res["occurrences"], 2);

        let (res, after) = str_replace_run("arr[x] = x", "[x]", "[y]").await;
        assert_eq!(after, "arr[y] = x");
        assert_eq!(res["occurrences"], 1);
    }

    #[tokio::test]
    async fn str_replace_dollar_patterns_are_literal() {
        let (_, after) = str_replace_run("price", "price", "$1").await;
        assert_eq!(after, "$1");
        let (_, after) = str_replace_run("price", "price", "$&$&").await;
        assert_eq!(after, "$&$&");
    }

    #[tokio::test]
    async fn str_replace_no_match_does_not_write() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("target.txt");
        std::fs::write(&file, "unchanged").unwrap();
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(946_684_800);
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(old)
            .unwrap();

        let res = StrReplaceTool::new(files_only(dir.path()))
            .invoke(map(
                json!({"file": "target.txt", "search": "nomatch", "replace": "X"}),
            ))
            .await
            .unwrap();

        assert_eq!(
            res,
            json!({"file": "target.txt", "replaced": false, "occurrences": 0})
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "unchanged");
        assert_eq!(std::fs::metadata(&file).unwrap().modified().unwrap(), old);
    }

    #[tokio::test]
    async fn str_replace_empty_search_is_no_match() {
        // TS: search === '' なら [content] として 0 件扱い
        let (res, after) = str_replace_run("abc", "", "X").await;
        assert_eq!(after, "abc");
        assert_eq!(res["replaced"], false);
        assert_eq!(res["occurrences"], 0);
    }

    #[tokio::test]
    async fn io_errors_include_path() {
        let dir = tempfile::tempdir().unwrap();
        let e = ViewTool::new(files_only(dir.path()))
            .invoke(map(json!({"path": "definitely/missing.txt"})))
            .await
            .unwrap_err();
        assert!(e.to_string().contains("definitely/missing.txt"), "{e}");
    }

    #[tokio::test]
    async fn missing_param_is_error() {
        let e = FileCreateTool::new(no_roots())
            .invoke(JsonMap::new())
            .await
            .unwrap_err();
        assert!(matches!(e, Error::InvalidParameter(_)));
    }

    #[tokio::test]
    async fn bash_is_disabled_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let e = BashTool::new(files_only(dir.path()))
            .invoke(map(json!({"command": "echo hi"})))
            .await
            .unwrap_err();
        assert!(
            e.to_string().contains("SUBAGENT_CODEGEN_ALLOWED_COMMANDS"),
            "{e}"
        );
    }

    #[cfg(windows)]
    const SHELL: (&str, &str, &str) = ("cmd", "cmd /c \"echo hi there\"", "cmd /c exit 3");
    #[cfg(not(windows))]
    const SHELL: (&str, &str, &str) = ("sh", "sh -c 'echo \"hi there\"'", "sh -c 'exit 3'");

    #[tokio::test]
    async fn bash_success_failure_and_quotes() {
        let dir = tempfile::tempdir().unwrap();
        let tool = BashTool::new(sandbox_in(dir.path(), CommandPolicy::allow(&[SHELL.0])));
        let ok = tool.invoke(map(json!({"command": SHELL.1}))).await.unwrap();
        assert_eq!(ok["success"], true, "{ok}");
        assert!(ok["stdout"].as_str().unwrap().contains("hi there"));

        let bad = tool.invoke(map(json!({"command": SHELL.2}))).await.unwrap();
        assert_eq!(bad["success"], false);
        assert!(
            bad["error"]
                .as_str()
                .unwrap()
                .starts_with(&format!("Command failed: {}", SHELL.2))
        );
        // shell の構文は引用符の外では受け付けない
        assert!(
            tool.invoke(map(json!({"command": format!("{} && echo x", SHELL.1)})))
                .await
                .is_err()
        );
        // cwd は workspace の中だけ
        assert!(
            tool.invoke(map(json!({"command": SHELL.1, "cwd": ".."})))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn lint_tool_parses_stdout_and_stderr() {
        #[cfg(windows)]
        let command = "cmd /c \"echo a.py:1:2: E1 out&& echo b.py:3:4: W2 err 1>&2\"";
        #[cfg(not(windows))]
        let command = "sh -c 'echo a.py:1:2: E1 out; echo b.py:3:4: W2 err 1>&2'";
        let dir = tempfile::tempdir().unwrap();
        let r = LintTool::new(sandbox_in(dir.path(), CommandPolicy::allow(&[SHELL.0])))
            .invoke(map(json!({ "command": command })))
            .await
            .unwrap();
        let issues = r["issues"].as_array().unwrap();
        assert_eq!(issues.len(), 2, "{r}");
        assert_eq!(issues[0]["file"], "a.py");
        assert_eq!(issues[0]["rule"], "E1");
        assert_eq!(issues[1]["file"], "b.py");
        assert_eq!(issues[1]["rule"], "W2");
        assert_eq!(issues[1]["type"], "warning");
    }

    #[test]
    fn lint_parsing() {
        let issues = parse_lint_output("ok\nfoo error x\nbar warning y\n");
        assert_eq!(
            issues,
            vec![
                json!({"type": "error", "message": "foo error x", "tool": "generic", "language": "unknown"}),
                json!({"type": "warning", "message": "bar warning y", "tool": "generic", "language": "unknown"}),
            ]
        );
    }

    #[test]
    fn display() {
        assert_eq!(js_display(None), "undefined");
        assert_eq!(js_display(Some(&json!("a"))), "a");
        assert_eq!(js_display(Some(&json!(1.5))), "1.5");
    }
}
