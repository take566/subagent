//! 組み込みツール実装（複数のエージェントで共有）

use crate::core::tool::{Tool, param_defs};
use crate::error::{Error, Result};
use crate::types::{JsonMap, js_len};
use async_trait::async_trait;
use regex::Regex;
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

/// lint 出力から `error` / `warning` を含む行を拾う
pub fn parse_lint_output(output: &str) -> Vec<Value> {
    output
        .split('\n')
        .filter(|l| l.contains("error") || l.contains("warning"))
        .map(|l| {
            json!({
                "type": if l.contains("error") { "error" } else { "warning" },
                "message": l,
            })
        })
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
/// `search` は正規表現（Rust の `regex` 構文。JS と違い後方参照・先読みは不可）、
/// `replace` は置換文字列（`$1` / `${name}` でグループ参照。JS の `$&` は `$0`）。
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
        let re = Regex::new(search)?;
        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| Error::io_at(&path, e))?;
        let occurrences = re.find_iter(&content).count();
        let new_content = js_replace_all(&re, &content, replace);
        tokio::fs::write(&path, new_content.as_bytes())
            .await
            .map_err(|e| Error::io_at(&path, e))?;
        Ok(json!({ "file": file, "replaced": true, "occurrences": occurrences }))
    }
}

/// JS の `String.prototype.replace(regex_g, string)` と同じ規則で置換する。
/// 置換文字列の `$$` `$&` `` $` `` `$'` `$n` `$nn` `$<name>` を解釈し、
/// それ以外の `$` は文字どおりに残す（Rust の `regex` の展開規則とは異なる）。
pub fn js_replace_all(re: &Regex, haystack: &str, replacement: &str) -> String {
    let group_count = re.captures_len() - 1;
    let has_named = re.capture_names().flatten().next().is_some();
    re.replace_all(haystack, |caps: &regex::Captures| {
        let m = caps.get(0).expect("group 0 always matches");
        let mut out = String::new();
        let bytes = replacement.as_bytes();
        let mut i = 0;
        while i < replacement.len() {
            let rest = &replacement[i..];
            if !rest.starts_with('$') || rest.len() == 1 {
                let ch = rest.chars().next().unwrap();
                out.push(ch);
                i += ch.len_utf8();
                continue;
            }
            match bytes[i + 1] {
                b'$' => {
                    out.push('$');
                    i += 2;
                }
                b'&' => {
                    out.push_str(m.as_str());
                    i += 2;
                }
                b'`' => {
                    out.push_str(&haystack[..m.start()]);
                    i += 2;
                }
                b'\'' => {
                    out.push_str(&haystack[m.end()..]);
                    i += 2;
                }
                d1 @ b'0'..=b'9' => {
                    let one = (d1 - b'0') as usize;
                    let two = bytes
                        .get(i + 2)
                        .filter(|b| b.is_ascii_digit())
                        .map(|d2| one * 10 + (d2 - b'0') as usize);
                    match two {
                        Some(n) if (1..=group_count).contains(&n) => {
                            out.push_str(caps.get(n).map_or("", |g| g.as_str()));
                            i += 3;
                        }
                        _ if (1..=group_count).contains(&one) => {
                            out.push_str(caps.get(one).map_or("", |g| g.as_str()));
                            i += 2;
                        }
                        _ => {
                            out.push('$');
                            i += 1;
                        }
                    }
                }
                b'<' if has_named => match rest.find('>') {
                    Some(end) => {
                        let name = &rest[2..end];
                        out.push_str(caps.name(name).map_or("", |g| g.as_str()));
                        i += end + 1;
                    }
                    None => {
                        out.push('$');
                        i += 1;
                    }
                },
                _ => {
                    out.push('$');
                    i += 1;
                }
            }
        }
        out
    })
    .into_owned()
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

        let r = StrReplaceTool::new(base)
            .invoke(map(
                json!({"file": "a/b.txt", "search": "fo+", "replace": "X"}),
            ))
            .await
            .unwrap();
        assert_eq!(r["occurrences"], 2);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "X X bar 日本");

        let v = ViewTool::default()
            .invoke(map(json!({"path": p.to_string_lossy()})))
            .await
            .unwrap();
        assert_eq!(v["lines"], 1);
        assert_eq!(v["content"], "X X bar 日本");
    }

    #[test]
    fn js_replacement_semantics() {
        let r =
            |pat: &str, hay: &str, rep: &str| js_replace_all(&Regex::new(pat).unwrap(), hay, rep);
        // 存在しないグループ参照や名前付き参照は文字どおり残る（JS と同じ）
        assert_eq!(r("foo", "x foo y", "${bar}"), "x ${bar} y");
        assert_eq!(r("foo", "x foo y", "cost: $price"), "x cost: $price y");
        assert_eq!(r("foo", "x foo y", "$1a"), "x $1a y");
        assert_eq!(r("foo", "x foo y", "<$<n>>"), "x <$<n>> y");
        assert_eq!(r("foo", "x foo y", "$"), "x $ y");
        // 特殊パターン
        assert_eq!(r("foo", "x foo y", "[$&]"), "x [foo] y");
        assert_eq!(r("foo", "x foo y", "$$"), "x $ y");
        assert_eq!(r("o", "ab o cd", "$`|$'"), "ab ab | cd cd");
        // グループ参照
        assert_eq!(r("(f)(o)o", "x foo y", "$2$1"), "x of y");
        assert_eq!(r("(f)", "f", "$10"), "f0");
        assert_eq!(r("(?<w>o+)", "foo", "[$<w>]"), "f[oo]");
        assert_eq!(r("(a)|(b)", "b", "[$1]"), "[]");
        assert_eq!(r("日", "日本", "$&語"), "日語本");
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
                json!({"type": "error", "message": "foo error x"}),
                json!({"type": "warning", "message": "bar warning y"}),
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
