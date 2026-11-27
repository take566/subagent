//! 組み込みツール実装（複数のエージェントで共有）

pub mod lint_parsers;
pub mod notion;

pub use notion::{NotionClient, NotionPageCreateTool, NotionPageUpdateTool};

use crate::core::tool::{Tool, param_defs};
use crate::error::{Error, Result};
use crate::types::{JsonMap, js_len};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Stdio;

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

/// `base_dir` 基準で絶対パスにする（TS の `path.resolve` 相当）
pub fn resolve_path(base_dir: Option<&Path>, p: &str) -> Result<PathBuf> {
    let joined = match base_dir {
        Some(base) => base.join(p),
        None => PathBuf::from(p),
    };
    Ok(std::path::absolute(joined)?)
}

// ---------------------------------------------------------------------------
// file_create
// ---------------------------------------------------------------------------

/// ファイル作成ツール
pub struct FileCreateTool {
    params: JsonMap,
    base_dir: Option<PathBuf>,
}

impl FileCreateTool {
    /// `base_dir` が None ならカレントディレクトリ基準
    pub fn new(base_dir: Option<PathBuf>) -> Self {
        FileCreateTool {
            params: param_defs(&[("path", "string", true), ("content", "string", true)]),
            base_dir,
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
        let path = required_str(&params, "path")?;
        let content = required_str(&params, "content")?;
        let full = resolve_path(self.base_dir.as_deref(), path)?;
        if let Some(dir) = full.parent() {
            tokio::fs::create_dir_all(dir)
                .await
                .map_err(|e| Error::io_at(dir, e))?;
        }
        tokio::fs::write(&full, content)
            .await
            .map_err(|e| Error::io_at(&full, e))?;
        Ok(json!({
            "path": full.to_string_lossy(),
            "created": true,
            "size": js_len(content),
        }))
    }
}

// ---------------------------------------------------------------------------
// bash_tool / lint
// ---------------------------------------------------------------------------

/// シェルコマンドの実行結果
#[derive(Debug, Clone)]
pub struct ShellOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    /// 失敗時のメッセージ（Node の `exec` と同じ `Command failed: ...` 形式）
    pub error: Option<String>,
}

/// シェル経由でコマンドを実行する（Node の `child_process.exec` 相当）。
/// Windows は `cmd.exe /d /s /c "<command>"`、それ以外は `/bin/sh -c`。
/// stdin は必ず null にする（MCP サーバーの stdin を子プロセスに奪わせない）。
pub async fn run_shell(command: &str, cwd: Option<&str>) -> Result<ShellOutput> {
    #[cfg(windows)]
    let mut cmd = {
        let mut c = tokio::process::Command::new("cmd.exe");
        c.args(["/d", "/s", "/c"]);
        c.raw_arg(format!("\"{command}\""));
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = tokio::process::Command::new("/bin/sh");
        c.args(["-c", command]);
        c
    };
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let out = match cmd.output().await {
        Ok(o) => o,
        Err(e) => {
            return Ok(ShellOutput {
                success: false,
                stdout: String::new(),
                stderr: String::new(),
                error: Some(e.to_string()),
            });
        }
    };
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let error = (!out.status.success()).then(|| format!("Command failed: {command}\n{stderr}"));
    Ok(ShellOutput {
        success: out.status.success(),
        stdout,
        stderr,
        error,
    })
}

/// Bash 実行ツール
pub struct BashTool {
    params: JsonMap,
}

impl Default for BashTool {
    fn default() -> Self {
        BashTool {
            params: param_defs(&[("command", "string", true), ("cwd", "string", false)]),
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
        let out = run_shell(command, optional_str(&params, "cwd")).await?;
        Ok(match out.error {
            None => json!({ "success": true, "stdout": out.stdout, "stderr": out.stderr }),
            Some(err) => json!({
                "success": false,
                "error": err,
                "stdout": out.stdout,
                "stderr": out.stderr,
            }),
        })
    }
}

/// Lint 実行ツール（ツール名は TS 版と同じく `bash_tool`）
pub struct LintTool {
    params: JsonMap,
}

impl Default for LintTool {
    fn default() -> Self {
        LintTool {
            params: param_defs(&[("command", "string", true), ("cwd", "string", false)]),
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
        let out = run_shell(command, optional_str(&params, "cwd")).await?;
        let issues = parse_lint_output(&out.stdout);
        Ok(match out.error {
            None => json!({
                "success": true,
                "stdout": out.stdout,
                "stderr": out.stderr,
                "issues": issues,
            }),
            Some(err) => json!({
                "success": false,
                "error": err,
                "stdout": out.stdout,
                "stderr": out.stderr,
                "issues": issues,
            }),
        })
    }
}

// ---------------------------------------------------------------------------
// str_replace
// ---------------------------------------------------------------------------

/// 文字列置換ツール。
///
/// `search` はリテラル文字列として扱い（正規表現ではない）、`replace` 内の `$1` `$&` なども
/// 置換パターンとして解釈せずそのまま書き込む（TS #53）。一致が 0 件ならファイルを書き戻さず
/// `replaced: false` を返す。
pub struct StrReplaceTool {
    params: JsonMap,
    base_dir: Option<PathBuf>,
}

impl StrReplaceTool {
    pub fn new(base_dir: Option<PathBuf>) -> Self {
        StrReplaceTool {
            params: param_defs(&[
                ("file", "string", true),
                ("search", "string", true),
                ("replace", "string", true),
            ]),
            base_dir,
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
        let file = required_str(&params, "file")?;
        let search = required_str(&params, "search")?;
        let replace = required_str(&params, "replace")?;
        let path = resolve_path(self.base_dir.as_deref(), file)?;
        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| Error::io_at(&path, e))?;
        // TS の split/join と同じく、重ならない一致を左から数える。空文字列は 0 件
        let occurrences = if search.is_empty() {
            0
        } else {
            content.matches(search).count()
        };
        if occurrences == 0 {
            return Ok(json!({ "file": file, "replaced": false, "occurrences": 0 }));
        }
        let new_content = content.replace(search, replace);
        tokio::fs::write(&path, new_content.as_bytes())
            .await
            .map_err(|e| Error::io_at(&path, e))?;
        Ok(json!({ "file": file, "replaced": true, "occurrences": occurrences }))
    }
}

// ---------------------------------------------------------------------------
// view
// ---------------------------------------------------------------------------

/// ファイル閲覧ツール
pub struct ViewTool {
    params: JsonMap,
}

impl Default for ViewTool {
    fn default() -> Self {
        ViewTool {
            params: param_defs(&[("path", "string", true)]),
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
        let path = required_str(&params, "path")?;
        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| Error::io_at(path, e))?;
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

    fn map(v: Value) -> JsonMap {
        v.as_object().unwrap().clone()
    }

    #[tokio::test]
    async fn file_create_and_str_replace_and_view() {
        let dir = tempfile::tempdir().unwrap();
        let base = Some(dir.path().to_path_buf());
        let r = FileCreateTool::new(base.clone())
            .invoke(map(
                json!({"path": "a/b.txt", "content": "foo foo bar 日本"}),
            ))
            .await
            .unwrap();
        assert_eq!(r["size"], 14); // UTF-16 単位（JS の length と同じ）
        let p = dir.path().join("a/b.txt");
        assert!(p.exists());

        // search はリテラル（TS #53）。`fo+` は正規表現として解釈されないので一致しない
        let r = StrReplaceTool::new(base.clone())
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

        let r = StrReplaceTool::new(base)
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

        let v = ViewTool::default()
            .invoke(map(json!({"path": p.to_string_lossy()})))
            .await
            .unwrap();
        assert_eq!(v["lines"], 1);
        assert_eq!(v["content"], "X X bar 日本");
    }

    // --- StrReplaceTool (literal replace)。TS tests/agents/codegen-agent.test.ts の移植 ---

    async fn str_replace_run(content: &str, search: &str, replace: &str) -> (Value, String) {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("target.txt");
        std::fs::write(&file, content).unwrap();
        let f = file.to_string_lossy().into_owned();
        let res = StrReplaceTool::new(None)
            .invoke(map(
                json!({"file": f, "search": search, "replace": replace}),
            ))
            .await
            .unwrap();
        let after = std::fs::read_to_string(&file).unwrap();
        // file は入力のまま返す
        assert_eq!(res["file"], json!(f));
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
        let f = file.to_string_lossy().into_owned();

        let res = StrReplaceTool::new(None)
            .invoke(map(json!({"file": f, "search": "nomatch", "replace": "X"})))
            .await
            .unwrap();

        assert_eq!(res, json!({"file": f, "replaced": false, "occurrences": 0}));
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
        let e = ViewTool::default()
            .invoke(map(json!({"path": "definitely/missing.txt"})))
            .await
            .unwrap_err();
        assert!(e.to_string().contains("definitely/missing.txt"), "{e}");
    }

    #[tokio::test]
    async fn missing_param_is_error() {
        let e = FileCreateTool::new(None)
            .invoke(JsonMap::new())
            .await
            .unwrap_err();
        assert!(matches!(e, Error::InvalidParameter(_)));
    }

    #[tokio::test]
    async fn bash_success_failure_and_quotes() {
        let ok = BashTool::default()
            .invoke(map(json!({"command": "echo \"hi there\""})))
            .await
            .unwrap();
        assert_eq!(ok["success"], true);
        assert!(ok["stdout"].as_str().unwrap().contains("hi there"));

        let bad = BashTool::default()
            .invoke(map(json!({"command": "exit 3"})))
            .await
            .unwrap();
        assert_eq!(bad["success"], false);
        assert!(
            bad["error"]
                .as_str()
                .unwrap()
                .starts_with("Command failed: exit 3")
        );
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
