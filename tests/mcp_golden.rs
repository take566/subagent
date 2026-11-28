//! MCP サーバー（bin）を子プロセスで起動して確かめる。
//!
//! - TS 版の記録（tests/golden/ts/*.json、history only）: `SUBAGENT_MCP_MODE=full` の direct tool と
//!   `execute` が移植元 TS 版（`29941ec`、@modelcontextprotocol/sdk 0.5.0）と同じ出力を返すこと。
//!   手順は tests/golden/README.md。
//! - Rust 版の golden（tests/golden/rust/*.json）: initialize、両 mode の tools/list、typed tool の
//!   エラーの形。`GOLDEN_BLESS=1` で書き直す（CI では失敗にする）。
//! - mode / deadline / progress / LLM backend の env の扱い。
//!
//! 子プロセスには API 鍵・LLM backend・mode 等の env を渡さず、proxy を到達できない trap に向ける
//! （[`server_command`]）。どの test も外へは通信しない。

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

/// 子プロセスの proxy の向け先。discard port で、何も listen していない（繋がらずに失敗する）
const PROXY_TRAP: &str = "http://127.0.0.1:9";

/// 子プロセスから外す env（開発機や CI の値を golden に混ぜない。鍵は渡さない）
const STRIPPED_ENV: &[&str] = &[
    "AGENT_ID",
    "MAX_CONCURRENT",
    "SUBAGENT_ROOTS",
    "SUBAGENT_CODEGEN_ALLOWED_COMMANDS",
    "SUBAGENT_COMMAND_TIMEOUT_SECS",
    "SUBAGENT_MAX_FILES",
    "SUBAGENT_MAX_FILE_BYTES",
    "SUBAGENT_ANTHROPIC_API_KEY",
    "ANTHROPIC_API_KEY",
    "SUBAGENT_LLM_BACKEND",
    "SUBAGENT_LLM_REPLAY_FILE",
    "SUBAGENT_MCP_MODE",
    "SUBAGENT_DEADLINE_SECS",
    "SUBAGENT_MAX_TOKENS_PER_DAY",
    "SUBAGENT_REVIEW_LINT_COMMAND",
    "SUBAGENT_MODEL",
    "SUBAGENT_EFFORT",
    "SUBAGENT_TOKEN_BUDGET",
    "NO_PROXY",
    "no_proxy",
    // 開発機や CI にトークンがあっても、golden の Notion 呼び出しで本物の API を叩かない
    "NOTION_TOKEN",
];

/// proxy の env（全部 trap に向ける）
const PROXY_ENV: &[&str] = &[
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
];

/// 子プロセスの Command。`envs` は外した後に足す
fn server_command(bin: &str, cwd: &Path, envs: &[(&str, &str)]) -> Command {
    let mut cmd = Command::new(bin);
    cmd.current_dir(cwd);
    for k in STRIPPED_ENV {
        cmd.env_remove(k);
    }
    // per-agent の上書き（SUBAGENT_<AGENT>_MODEL など）も含め、SUBAGENT_* / ANTHROPIC_* は全部外す
    for (k, _) in std::env::vars_os() {
        let k = k.to_string_lossy();
        if k.starts_with("SUBAGENT_") || k.starts_with("ANTHROPIC_") {
            cmd.env_remove(k.as_ref());
        }
    }
    cmd.env("SUBAGENT_LOG_LEVEL", "debug"); // ログが stdout を汚さないことも確かめる
    for k in PROXY_ENV {
        cmd.env(k, PROXY_TRAP);
    }
    cmd.envs(envs.iter().copied());
    cmd
}

struct Client {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    non_json_lines: Vec<String>,
    /// 受け取った通知（method と params）
    notifications: Vec<Value>,
    /// 待っていた id 以外の応答
    other_responses: Vec<Value>,
}

impl Client {
    /// 作業ディレクトリを workspace root（`SUBAGENT_ROOTS`）にして、`SUBAGENT_MCP_MODE=full` で起動する
    fn spawn(bin: &str, cwd: &Path) -> Client {
        Self::spawn_rooted(bin, cwd, &[("SUBAGENT_MCP_MODE", "full")])
    }

    /// 作業ディレクトリを workspace root にして起動する（mode などは `envs` で渡す）
    fn spawn_rooted(bin: &str, cwd: &Path, envs: &[(&str, &str)]) -> Client {
        let root = std::path::absolute(cwd).unwrap();
        let root = root.to_string_lossy().into_owned();
        let mut all = vec![("SUBAGENT_ROOTS", root.as_str())];
        all.extend_from_slice(envs);
        Self::spawn_with(bin, cwd, &all, Stdio::null())
    }

    fn spawn_with(bin: &str, cwd: &Path, envs: &[(&str, &str)], stderr: Stdio) -> Client {
        let mut child = server_command(bin, cwd, envs)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr)
            .spawn()
            .expect("spawn server");
        Client {
            stdin: child.stdin.take().unwrap(),
            stdout: BufReader::new(child.stdout.take().unwrap()),
            child,
            next_id: 0,
            non_json_lines: vec![],
            notifications: vec![],
            other_responses: vec![],
        }
    }

    fn send(&mut self, msg: Value) {
        writeln!(self.stdin, "{msg}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn initialize(&mut self) -> Value {
        let r = self.request(
            "initialize",
            json!({ "protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": { "name": "t", "version": "0" } }),
        );
        self.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        r
    }

    /// tools/call して (isError, text) を返す
    fn call(&mut self, name: &str, arguments: Value) -> (bool, String) {
        let r = self.request(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        );
        (
            r["isError"].as_bool().unwrap_or(false),
            r["content"][0]["text"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        )
    }

    fn tool_names(&mut self) -> Vec<String> {
        let list = self.request("tools/list", json!({}));
        list["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect()
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            let mut line = String::new();
            assert!(
                self.stdout.read_line(&mut line).unwrap() > 0,
                "server closed stdout"
            );
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match serde_json::from_str::<Value>(line) {
                Ok(msg) if msg.get("id") == Some(&json!(id)) => {
                    assert!(msg.get("error").is_none(), "{method} failed: {msg}");
                    return msg["result"].clone();
                }
                Ok(msg) if msg.get("id").is_none() && msg.get("method").is_some() => {
                    self.notifications.push(msg);
                }
                Ok(msg) => self.other_responses.push(msg),
                Err(_) => self.non_json_lines.push(line.to_string()),
            }
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// 実行ごとに変わる値を正規化
fn normalize(v: Value, work: &str) -> Value {
    match v {
        Value::String(s) => {
            let s = s.replace(work, "<WORK>").replace('\\', "/");
            let re = regex::Regex::new(r"generated-\d{4}-\d\d-\d\dT\d\d-\d\d-\d\d-\d{3}Z").unwrap();
            let s = re.replace_all(&s, "generated-<TIMESTAMP>").into_owned();
            let uuid =
                regex::Regex::new(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")
                    .unwrap();
            Value::String(uuid.replace_all(&s, "<UUID>").into_owned())
        }
        Value::Array(a) => Value::Array(a.into_iter().map(|x| normalize(x, work)).collect()),
        Value::Object(m) => Value::Object(
            m.into_iter()
                .map(|(k, x)| {
                    let x = if k == "duration_ms" {
                        json!("<DURATION>")
                    } else {
                        normalize(x, work)
                    };
                    (k, x)
                })
                .collect(),
        ),
        other => other,
    }
}

/// tools/call のテキストを比較可能な値にする（JSON ならパース、そうでなければ文字列のまま）
fn text_value(text: &str, work: &str) -> Value {
    match serde_json::from_str::<Value>(text) {
        Ok(v) => normalize(v, work),
        Err(_) => json!(text),
    }
}

/// 各サーバーの typed tool の名前
fn typed_name(agent: &str) -> &'static str {
    match agent {
        "research" => "research",
        "codegen" => "codegen",
        "review" => "review",
        "document" => "document",
        _ => unreachable!(),
    }
}

/// `bash_tool`（CodeGen）の説明。TS 版は「Bashコマンドを実行」だったが、shell は使わない（意図した差分）
const BASH_TOOL_DESCRIPTION: &str = "許可された program を shell を通さずに実行する（SUBAGENT_CODEGEN_ALLOWED_COMMANDS にある名前だけ。command は program と引数）";

fn check_server_against_ts(agent: &str, bin: &str) {
    let golden: Value = serde_json::from_str(
        &std::fs::read_to_string(format!(
            "{}/tests/golden/ts/{agent}.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap(),
    )
    .unwrap();
    let work_dir = tempfile::tempdir().unwrap();
    let work = std::path::absolute(work_dir.path())
        .unwrap()
        .to_string_lossy()
        .into_owned();

    let mut c = Client::spawn(bin, work_dir.path());
    let init = c.initialize();
    assert_eq!(
        init["serverInfo"]["name"], golden["serverInfo"]["name"],
        "{agent}: serverInfo.name"
    );
    assert_eq!(
        init["serverInfo"]["version"], golden["serverInfo"]["version"],
        "{agent}: serverInfo.version"
    );

    // tools/list（full）: TS 版のツールが同じ順に並び、その後ろに typed tool が 1 つだけ付く
    let list = c.request("tools/list", json!({}));
    let pick = |tools: &[Value]| -> Vec<Value> {
        tools
            .iter()
            .map(|t| json!({ "name": t["name"], "description": t["description"], "inputSchema": t["inputSchema"] }))
            .collect()
    };
    let got_tools = list["tools"].as_array().unwrap();
    let (typed, direct) = got_tools.split_last().unwrap();
    assert_eq!(typed["name"], typed_name(agent), "{agent}: typed tool last");
    let mut expected_tools = pick(golden["tools"].as_array().unwrap());
    for t in &mut expected_tools {
        if agent == "codegen" && t["name"] == "bash_tool" {
            // 意図した差分: shell を使わなくなったので説明を直した
            assert_eq!(t["description"], "Bashコマンドを実行");
            t["description"] = json!(BASH_TOOL_DESCRIPTION);
        }
    }
    assert_eq!(pick(direct), expected_tools, "{agent}: tools/list");

    // tools/call
    for case in golden["calls"].as_array().unwrap() {
        let call = &case["call"];
        let name = call["name"].as_str().unwrap();
        let got = c.request("tools/call", call.clone());
        let expected = &case["response"];

        let got_err = got["isError"].as_bool().unwrap_or(false);
        let exp_err = expected["isError"].as_bool().unwrap_or(false);
        assert_eq!(got_err, exp_err, "{agent}/{name}: isError");

        let got_v = text_value(got["content"][0]["text"].as_str().unwrap(), &work);
        let mut exp_v = text_value(expected["content"][0]["text"].as_str().unwrap(), "<WORK>");
        if agent == "review" {
            // 意図した差分: TS 版は `npm run lint` を固定実行し、npm のエラー行を lint エラーとして
            // 数えていた（score 20）。Rust 版は lint_command 指定時のみ lint を実行する。
            exp_v["result"]["issues"] = json!([]);
            exp_v["result"]["score"] = json!(100);
            exp_v["metrics"]["tools_called"] = json!(0);
        }
        if agent == "document"
            && name.starts_with("notion_")
            && let Some(text) = exp_v.as_str().filter(|t| t.ends_with("is required"))
        {
            // 意図した差分（CHANGELOG 1.1.0）: Notion ツールの引数エラーには `Invalid parameter: ` が付く
            exp_v = json!(text.replacen("Error: ", "Error: Invalid parameter: ", 1));
        }
        assert_eq!(got_v, exp_v, "{agent}/{name}: result");
    }

    // TS 版は console.info のログで stdout を汚していた（golden 記録時 45 行以上）。Rust 版は 0 行。
    assert!(
        c.non_json_lines.is_empty(),
        "{agent}: non-JSON stdout lines: {:?}",
        c.non_json_lines
    );
}

#[test]
fn research_server_matches_ts() {
    check_server_against_ts("research", env!("CARGO_BIN_EXE_research-server"));
}

#[test]
fn codegen_server_matches_ts() {
    check_server_against_ts("codegen", env!("CARGO_BIN_EXE_codegen-server"));
}

#[test]
fn review_server_matches_ts() {
    check_server_against_ts("review", env!("CARGO_BIN_EXE_review-server"));
}

#[test]
fn document_server_matches_ts() {
    check_server_against_ts("document", env!("CARGO_BIN_EXE_document-server"));
}

// ---------------------------------------------------------------------------
// Rust 版の golden（initialize / tools/list 両 mode / typed tool のエラーの形）
// ---------------------------------------------------------------------------

/// typed tool に送るエラーのケース（golden に call と response を残す）
fn typed_error_cases(agent: &str) -> Vec<(&'static str, Value)> {
    let name = typed_name(agent);
    let valid = match agent {
        "research" => json!({ "query": "rust", "depth": "quick" }),
        "codegen" => json!({ "task": "add two numbers" }),
        "review" => json!({ "code": "fn main() {}" }),
        "document" => json!({ "content": "# T", "format": "md" }),
        _ => unreachable!(),
    };
    let mut with_extra = valid.clone();
    with_extra["nope"] = json!(1);
    let mut cases = vec![
        (
            "unexpected_field",
            json!({ "name": name, "arguments": with_extra }),
        ),
        (
            "wrong_type",
            json!({ "name": name, "arguments": match agent {
                "research" => json!({ "query": 1 }),
                "codegen" => json!({ "task": "x", "allow_commands": "yes" }),
                "review" => json!({ "code": "x", "lint": "yes" }),
                _ => json!({ "content": "x", "format": "pdf" }),
            } }),
        ),
        (
            "not_configured",
            json!({ "name": name, "arguments": valid }),
        ),
    ];
    if agent != "review" {
        cases.push(("missing_required", json!({ "name": name, "arguments": {} })));
    }
    // 引数そのものが無い
    cases.push(("missing_arguments", json!({ "name": name })));
    if agent == "review" {
        cases.push((
            "code_and_path",
            json!({ "name": name, "arguments": { "code": "x", "path": "a.rs" } }),
        ));
        cases.push((
            "neither_code_nor_path",
            json!({ "name": name, "arguments": {} }),
        ));
    }
    cases
}

fn bless() -> bool {
    let set = std::env::var("GOLDEN_BLESS").is_ok_and(|v| !v.is_empty() && v != "0");
    assert!(
        !(set && std::env::var_os("CI").is_some()),
        "GOLDEN_BLESS must not be set in CI"
    );
    set
}

fn check_rust_golden(agent: &str, bin: &str) {
    let dir = tempfile::tempdir().unwrap();
    let work = std::path::absolute(dir.path())
        .unwrap()
        .to_string_lossy()
        .into_owned();

    // 既定（hermes）
    let mut c = Client::spawn_rooted(bin, dir.path(), &[]);
    let init = c.initialize();
    let hermes = c.request("tools/list", json!({}));
    let mut typed_errors = vec![];
    for (label, call) in typed_error_cases(agent) {
        let response = c.request("tools/call", call.clone());
        typed_errors
            .push(json!({ "case": label, "call": call, "response": normalize(response, &work) }));
    }
    drop(c);

    let mut c = Client::spawn_rooted(bin, dir.path(), &[("SUBAGENT_MCP_MODE", "full")]);
    c.initialize();
    let full = c.request("tools/list", json!({}));

    let got = json!({
        "initialize": init,
        "tools_list": { "hermes": hermes["tools"], "full": full["tools"] },
        "typed_errors": typed_errors,
    });
    let path = format!(
        "{}/tests/golden/rust/{agent}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    if bless() {
        std::fs::write(&path, serde_json::to_string_pretty(&got).unwrap() + "\n").unwrap();
        return;
    }
    let expected: Value = serde_json::from_str(
        &std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{path}: {e} (run with GOLDEN_BLESS=1 to record)")),
    )
    .unwrap();
    assert_eq!(
        got, expected,
        "{agent}: differs from {path} (re-record with GOLDEN_BLESS=1 if intended)"
    );
}

#[test]
fn research_server_matches_rust_golden() {
    check_rust_golden("research", env!("CARGO_BIN_EXE_research-server"));
}

#[test]
fn codegen_server_matches_rust_golden() {
    check_rust_golden("codegen", env!("CARGO_BIN_EXE_codegen-server"));
}

#[test]
fn review_server_matches_rust_golden() {
    check_rust_golden("review", env!("CARGO_BIN_EXE_review-server"));
}

#[test]
fn document_server_matches_rust_golden() {
    check_rust_golden("document", env!("CARGO_BIN_EXE_document-server"));
}

// ---------------------------------------------------------------------------
// SUBAGENT_MCP_MODE
// ---------------------------------------------------------------------------

fn all_bins() -> [(&'static str, &'static str); 4] {
    [
        ("research", env!("CARGO_BIN_EXE_research-server")),
        ("codegen", env!("CARGO_BIN_EXE_codegen-server")),
        ("review", env!("CARGO_BIN_EXE_review-server")),
        ("document", env!("CARGO_BIN_EXE_document-server")),
    ]
}

/// 既定（hermes）と不明な値では typed tool だけを公開し、direct tool と execute の呼び出しは拒否する
#[test]
fn hermes_mode_lists_only_the_typed_tool_and_rejects_direct_tools() {
    let direct = |agent: &str| -> Vec<(&'static str, Value)> {
        let mut v = vec![(
            "execute",
            json!({ "action": agent, "parameters": { "query": "q", "specification": "s", "source_code": "x", "content": "c" } }),
        )];
        v.extend(match agent {
            "research" => vec![("web_search", json!({ "query": "q" }))],
            "codegen" => vec![
                ("file_create", json!({ "path": "a.txt", "content": "x" })),
                ("bash_tool", json!({ "command": "echo hi" })),
            ],
            "review" => vec![("view", json!({ "path": "a.rs" }))],
            _ => vec![("file_create", json!({ "path": "a.txt", "content": "x" }))],
        });
        v
    };
    for mode in [None, Some("hermes"), Some("FULLX"), Some("")] {
        for (agent, bin) in all_bins() {
            let dir = tempfile::tempdir().unwrap();
            let envs: Vec<(&str, &str)> =
                mode.map(|m| ("SUBAGENT_MCP_MODE", m)).into_iter().collect();
            let mut c = Client::spawn_rooted(bin, dir.path(), &envs);
            c.initialize();
            assert_eq!(c.tool_names(), vec![typed_name(agent)], "{agent} {mode:?}");
            for (tool, args) in direct(agent) {
                let (err, text) = c.call(tool, args);
                assert!(err, "{agent} {mode:?} {tool}: {text}");
                assert!(text.contains("SUBAGENT_MCP_MODE"), "{agent} {tool}: {text}");
            }
            // 何も書いていない
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "{agent}");
        }
    }
}

/// full は direct tool + execute + typed tool。値の前後の空白と大文字小文字は問わない
#[test]
fn full_mode_lists_direct_tools_and_the_typed_tool() {
    for value in ["full", " FULL "] {
        let dir = tempfile::tempdir().unwrap();
        let mut c = Client::spawn_rooted(
            env!("CARGO_BIN_EXE_codegen-server"),
            dir.path(),
            &[("SUBAGENT_MCP_MODE", value)],
        );
        c.initialize();
        assert_eq!(
            c.tool_names(),
            [
                "file_create",
                "bash_tool",
                "str_replace",
                "execute",
                "codegen"
            ],
            "{value:?}"
        );
    }
}

/// LLM が未設定（鍵なし）なら typed tool は mock の出力を返さず isError にする
#[test]
fn typed_tool_without_llm_is_an_error() {
    for (agent, bin) in all_bins() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = Client::spawn_rooted(bin, dir.path(), &[]);
        c.initialize();
        let args = match agent {
            "research" => json!({ "query": "rust" }),
            "codegen" => json!({ "task": "x" }),
            "review" => json!({ "code": "fn a() {}" }),
            _ => json!({ "content": "x" }),
        };
        let (err, text) = c.call(typed_name(agent), args);
        assert!(err, "{agent}: {text}");
        assert!(text.contains("LLM is not configured"), "{agent}: {text}");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "{agent}");
    }
}

// ---------------------------------------------------------------------------
// notifications/progress
// ---------------------------------------------------------------------------

fn progress_of(c: &Client, token: &Value) -> Vec<f64> {
    c.notifications
        .iter()
        .filter(|n| n["method"] == "notifications/progress")
        .filter(|n| &n["params"]["progressToken"] == token)
        .map(|n| n["params"]["progress"].as_f64().unwrap())
        .collect()
}

#[test]
fn progress_is_sent_only_with_a_token_and_strictly_increases() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = Client::spawn_rooted(
        env!("CARGO_BIN_EXE_review-server"),
        dir.path(),
        &[("SUBAGENT_MCP_MODE", "full")],
    );
    c.initialize();

    // token 無し: 通知なし
    let r = c.request(
        "tools/call",
        json!({ "name": "execute", "arguments": { "action": "review", "parameters": { "source_code": "x" } } }),
    );
    assert!(r.get("isError").is_none_or(|v| v == false), "{r}");
    c.request("ping", json!({}));
    assert!(
        c.notifications
            .iter()
            .all(|n| n["method"] != "notifications/progress"),
        "{:?}",
        c.notifications
    );

    // token 付き: 応答より前に、増え続ける progress が少なくとも開始と終了の 2 回
    for token in [json!("tok-1"), json!(7)] {
        let r = c.request(
            "tools/call",
            json!({ "name": "execute", "_meta": { "progressToken": token }, "arguments": { "action": "review", "parameters": { "source_code": "x" } } }),
        );
        assert!(r.get("isError").is_none_or(|v| v == false), "{r}");
        let before = progress_of(&c, &token);
        assert!(before.len() >= 2, "{token}: {before:?}");
        assert!(
            before.windows(2).all(|w| w[0] < w[1]),
            "{token}: {before:?}"
        );
        // 応答の後には来ない
        c.request("ping", json!({}));
        assert_eq!(progress_of(&c, &token), before, "{token}");
    }
    // エラーでも終了の通知は来る
    let token = json!("tok-err");
    let r = c.request(
        "tools/call",
        json!({ "name": "review", "_meta": { "progressToken": token }, "arguments": { "code": "x" } }),
    );
    assert_eq!(r["isError"], true, "{r}");
    assert!(progress_of(&c, &token).len() >= 2);
}

// ---------------------------------------------------------------------------
// 子プロセスの env と LLM backend
// ---------------------------------------------------------------------------

/// golden の子プロセスには鍵・backend・mode 等を渡さず、proxy を trap に向ける
#[test]
fn child_processes_have_keys_stripped_and_proxy_trapped() {
    let dir = tempfile::tempdir().unwrap();
    let cmd = server_command(env!("CARGO_BIN_EXE_research-server"), dir.path(), &[]);
    let envs: std::collections::HashMap<String, Option<String>> = cmd
        .get_envs()
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.map(|v| v.to_string_lossy().into_owned()),
            )
        })
        .collect();
    for k in [
        "SUBAGENT_ANTHROPIC_API_KEY",
        "ANTHROPIC_API_KEY",
        "SUBAGENT_LLM_BACKEND",
        "SUBAGENT_LLM_REPLAY_FILE",
        "SUBAGENT_MCP_MODE",
        "SUBAGENT_DEADLINE_SECS",
        "SUBAGENT_MAX_TOKENS_PER_DAY",
        "NOTION_TOKEN",
        "NO_PROXY",
    ] {
        assert_eq!(envs.get(k), Some(&None), "{k} must be removed");
    }
    for k in ["HTTPS_PROXY", "HTTP_PROXY", "ALL_PROXY"] {
        assert_eq!(envs.get(k), Some(&Some(PROXY_TRAP.to_string())), "{k}");
    }
    // 環境にあった SUBAGENT_* / ANTHROPIC_* は全部外す
    for (k, _) in std::env::vars() {
        if (k.starts_with("SUBAGENT_") && k != "SUBAGENT_LOG_LEVEL") || k.starts_with("ANTHROPIC_")
        {
            assert_eq!(envs.get(&k), Some(&None), "{k} must be removed");
        }
    }
    // trap は本当に繋がらない
    assert!(
        std::net::TcpStream::connect_timeout(
            &"127.0.0.1:9".parse().unwrap(),
            Duration::from_secs(2)
        )
        .is_err(),
        "something listens on the proxy trap port"
    );
}

/// 子プロセスが終わるのを待つ（終わらなければ kill して None）
fn wait_exit(child: &mut Child, limit: Duration) -> Option<std::process::ExitStatus> {
    let started = Instant::now();
    while started.elapsed() < limit {
        if let Some(status) = child.try_wait().unwrap() {
            return Some(status);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    None
}

/// 起動時に失敗して終わる（stdin は開いたまま）。終了 status と stderr を返す
fn spawn_expect_exit(bin: &str, envs: &[(&str, &str)]) -> (std::process::ExitStatus, String) {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("stderr.log");
    let log = std::fs::File::create(&log_path).unwrap();
    let mut child = server_command(bin, dir.path(), envs)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log))
        .spawn()
        .unwrap();
    let status = wait_exit(&mut child, Duration::from_secs(20))
        .unwrap_or_else(|| panic!("{bin} {envs:?} kept running"));
    (status, std::fs::read_to_string(&log_path).unwrap())
}

/// 不明な backend は起動せずに失敗する（fail closed）
#[test]
fn unknown_llm_backend_exits_non_zero() {
    for (_, bin) in all_bins() {
        let (status, stderr) = spawn_expect_exit(bin, &[("SUBAGENT_LLM_BACKEND", "openai")]);
        assert_eq!(status.code(), Some(2), "{bin}: {status}");
        assert!(stderr.contains("SUBAGENT_LLM_BACKEND"), "{stderr}");
    }
}

/// `replay` feature 無しの bin は SUBAGENT_LLM_BACKEND=replay で起動しない
#[cfg(not(feature = "replay"))]
#[test]
fn replay_backend_without_feature_exits_non_zero() {
    for (_, bin) in all_bins() {
        let (status, stderr) = spawn_expect_exit(bin, &[("SUBAGENT_LLM_BACKEND", "replay")]);
        assert_eq!(status.code(), Some(2), "{bin}: {status}");
        assert!(stderr.contains("replay"), "{stderr}");
    }
}

/// 不正な per-agent の設定でも起動しない
/// 不正・0・大きすぎる（> 86400）deadline では起動しない（以前は u64::MAX で Instant の加算が panic した）
#[test]
fn invalid_deadline_exits_non_zero() {
    for v in ["abc", "300s", "0", "-1", "86401", "18446744073709551615"] {
        let (status, stderr) = spawn_expect_exit(
            env!("CARGO_BIN_EXE_review-server"),
            &[("SUBAGENT_DEADLINE_SECS", v)],
        );
        assert_eq!(status.code(), Some(2), "{v}: {status}");
        assert!(stderr.contains("SUBAGENT_DEADLINE_SECS"), "{v}: {stderr}");
    }
}

#[cfg(windows)]
const SLOW: (&str, &str) = ("ping", "ping -n 30 127.0.0.1");
#[cfg(not(windows))]
const SLOW: (&str, &str) = ("sleep", "sleep 30");

/// `notifications/cancelled` で実行中の tools/call を止める: 応答は返さず、同時実行数の枠も
/// すぐ空く（MAX_CONCURRENT=1 なので、止まらなければ次の呼び出しは 30 秒待つ）
#[test]
fn cancelled_request_frees_the_slot_and_gets_no_response() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = Client::spawn_rooted(
        env!("CARGO_BIN_EXE_codegen-server"),
        dir.path(),
        &[
            FULL,
            ("MAX_CONCURRENT", "1"),
            ("SUBAGENT_CODEGEN_ALLOWED_COMMANDS", SLOW.0),
        ],
    );
    c.initialize();
    c.next_id += 1;
    let slow_id = c.next_id;
    c.send(
        json!({ "jsonrpc": "2.0", "id": slow_id, "method": "tools/call",
        "params": { "name": "bash_tool", "arguments": { "command": SLOW.1 } } }),
    );
    // 実行が始まってから取り消す
    std::thread::sleep(Duration::from_secs(2));
    c.send(
        json!({ "jsonrpc": "2.0", "method": "notifications/cancelled",
        "params": { "requestId": slow_id, "reason": "test" } }),
    );
    let started = Instant::now();
    let (err, text) = c.call("file_create", json!({ "path": "a.txt", "content": "x" }));
    assert!(!err, "{text}");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the cancelled call kept the slot: {:?}",
        started.elapsed()
    );
    // 少し待ってから、取り消した呼び出しの応答が来ていないことを確かめる
    std::thread::sleep(Duration::from_secs(1));
    c.request("ping", json!({}));
    assert!(
        c.other_responses
            .iter()
            .all(|m| m.get("id") != Some(&json!(slow_id))),
        "{:?}",
        c.other_responses
    );
}

#[test]
fn invalid_agent_llm_settings_exit_non_zero() {
    for (envs, needle) in [
        (&[("SUBAGENT_EFFORT", "extreme")][..], "SUBAGENT_EFFORT"),
        (
            &[("SUBAGENT_RESEARCH_TOKEN_BUDGET", "lots")][..],
            "SUBAGENT_RESEARCH_TOKEN_BUDGET",
        ),
    ] {
        let (status, stderr) = spawn_expect_exit(env!("CARGO_BIN_EXE_research-server"), envs);
        assert_eq!(status.code(), Some(2), "{envs:?}: {status}");
        assert!(stderr.contains(needle), "{stderr}");
    }
}

/// replay backend: 記録した応答（ここでは空）で typed tool が動き、NotConfigured にならない
#[cfg(feature = "replay")]
#[test]
fn replay_backend_serves_typed_tools() {
    let fixture = format!(
        "{}/tests/golden/replay/empty.json",
        env!("CARGO_MANIFEST_DIR")
    );
    for (agent, bin) in all_bins() {
        // research は空の記録では答えられない（LLM の経路。tests/research_agent.rs の replay_fixture_succeeds）
        if agent == "research" {
            continue;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut c = Client::spawn_rooted(
            bin,
            dir.path(),
            &[
                ("SUBAGENT_LLM_BACKEND", "replay"),
                ("SUBAGENT_LLM_REPLAY_FILE", &fixture),
                ("MAX_CONCURRENT", "1"),
            ],
        );
        c.initialize();
        let args = match agent {
            "research" => json!({ "query": "rust" }),
            "codegen" => json!({ "task": "x" }),
            "review" => json!({ "code": "// c\nfn a() {}" }),
            _ => json!({ "content": "x", "format": "txt" }),
        };
        let (err, text) = c.call(typed_name(agent), args);
        assert!(!err, "{agent}: {text}");
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["status"], "completed", "{agent}: {v}");
    }
    // 記録ファイルが無ければ起動しない
    let (status, stderr) = spawn_expect_exit(
        env!("CARGO_BIN_EXE_review-server"),
        &[("SUBAGENT_LLM_BACKEND", "replay")],
    );
    assert_eq!(status.code(), Some(2), "{status}");
    assert!(stderr.contains("SUBAGENT_LLM_REPLAY_FILE"), "{stderr}");
}

// ---------------------------------------------------------------------------
// 既存の direct tool（SUBAGENT_MCP_MODE=full）
// ---------------------------------------------------------------------------

/// TS 版でも isError になる入力（golden 記録後に追加した確認）
#[test]
fn invalid_inputs_are_tool_errors() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = Client::spawn(env!("CARGO_BIN_EXE_review-server"), dir.path());
    c.initialize();
    // TS #54: execute の引数は agent に渡す前に検証する
    for (args, expected) in [
        (
            json!({ "task_id": "r1", "parameters": {} }),
            "Error: Invalid arguments: action must be a non-empty string",
        ),
        (
            json!({ "parameters": { "source_code": 42 } }),
            "Error: Invalid arguments: action must be a non-empty string",
        ),
        // 以前は数値の task_id を文字列化して通していた
        (
            json!({ "task_id": 7, "action": "review", "parameters": { "source_code": "x" } }),
            "Error: Invalid arguments: task_id must be a non-empty string",
        ),
        (
            json!({ "action": "review", "parameters": "x" }),
            "Error: Invalid arguments: parameters must be an object",
        ),
    ] {
        let r = c.request(
            "tools/call",
            json!({ "name": "execute", "arguments": args }),
        );
        assert_eq!(r["isError"], true, "{r}");
        assert_eq!(r["content"][0]["text"], expected, "{args}");
    }
    // 引数が正しくても agent 側で失敗すれば isError（検証のエラーではない）
    let r = c.request(
        "tools/call",
        json!({ "name": "execute", "arguments": { "action": "review", "parameters": { "source_code": 42 } } }),
    );
    assert_eq!(r["isError"], true, "{r}");
    let text = r["content"][0]["text"].as_str().unwrap();
    assert!(
        text.starts_with("Error: ") && !text.contains("Invalid arguments"),
        "{text}"
    );
    let r = c.request("tools/call", json!({ "name": "execute" }));
    assert_eq!(r["content"][0]["text"], "Error: Missing arguments");
}

/// TS #40: AGENT_ID は agent の id（MCP ログの agent_id）に反映され、MAX_CONCURRENT を付けても動く
#[test]
fn agent_id_env_is_applied() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("stderr.log");
    {
        let log = std::fs::File::create(&log_path).unwrap();
        let mut c = Client::spawn_with(
            env!("CARGO_BIN_EXE_research-server"),
            dir.path(),
            &[
                ("AGENT_ID", " research-xyz "),
                ("MAX_CONCURRENT", "1"),
                ("SUBAGENT_MCP_MODE", "full"),
            ],
            Stdio::from(log),
        );
        c.initialize();
        let r = c.request(
            "tools/call",
            json!({ "name": "execute", "arguments": { "task_id": "e1", "action": "research", "parameters": { "query": "q" } } }),
        );
        assert!(r.get("isError").is_none_or(|v| v == false), "{r}");
        assert!(c.non_json_lines.is_empty(), "{:?}", c.non_json_lines);
    } // drop で子プロセスを止める
    let log = std::fs::read_to_string(&log_path).unwrap();
    assert!(
        log.contains("\"agent_id\": \"mcp-research-xyz\""),
        "MCP logger id: {log}"
    );
    assert!(
        log.contains("\"agent_id\": \"research-xyz\""),
        "agent logger id: {log}"
    );
    // タスク中のツールログには task_id が載る（TS #44）
    assert!(log.contains("\"task_id\": \"e1\""), "task_id: {log}");
}

// ---------------------------------------------------------------------------
// sandbox（SUBAGENT_ROOTS / SUBAGENT_CODEGEN_ALLOWED_COMMANDS）
// ---------------------------------------------------------------------------

const FULL: (&str, &str) = ("SUBAGENT_MCP_MODE", "full");

/// roots 未設定なら書き込み系のツールは拒否し、cwd にも書かない
#[test]
fn codegen_without_roots_refuses_writes() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = Client::spawn_with(
        env!("CARGO_BIN_EXE_codegen-server"),
        dir.path(),
        &[FULL],
        Stdio::null(),
    );
    c.initialize();
    let (err, text) = c.call(
        "execute",
        json!({ "action": "codegen", "parameters": { "specification": "x" } }),
    );
    assert!(err, "{text}");
    assert!(text.contains("SUBAGENT_ROOTS"), "{text}");
    let (err, text) = c.call("file_create", json!({ "path": "a.txt", "content": "x" }));
    assert!(err && text.contains("SUBAGENT_ROOTS"), "{text}");
    let (err, text) = c.call(
        "str_replace",
        json!({ "file": "a.txt", "search": "x", "replace": "y" }),
    );
    assert!(err && text.contains("SUBAGENT_ROOTS"), "{text}");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

/// review の view も roots 未設定なら読まない
#[test]
fn review_view_without_roots_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "fn a() {}").unwrap();
    let mut c = Client::spawn_with(
        env!("CARGO_BIN_EXE_review-server"),
        dir.path(),
        &[FULL],
        Stdio::null(),
    );
    c.initialize();
    let (err, text) = c.call("view", json!({ "path": "a.rs" }));
    assert!(err && text.contains("SUBAGENT_ROOTS"), "{text}");
}

/// workspace の外（絶対 path / `..`）は読み書きしない
#[test]
fn paths_outside_workspace_are_refused() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("secret.txt");
    std::fs::write(&secret, "top secret").unwrap();
    let abs = secret.to_string_lossy().into_owned();

    let mut c = Client::spawn(env!("CARGO_BIN_EXE_review-server"), root.path());
    c.initialize();
    let (err, text) = c.call("view", json!({ "path": abs }));
    assert!(err && !text.contains("top secret"), "{text}");
    let (err, text) = c.call(
        "execute",
        json!({ "action": "review", "parameters": { "source_code": abs } }),
    );
    assert!(err && !text.contains("top secret"), "{text}");
    // workspace 内のファイルは `source_path` で読める
    std::fs::write(root.path().join("in.rs"), "// c\nfn a() {}\n").unwrap();
    let (err, text) = c.call(
        "execute",
        json!({ "action": "review", "parameters": { "source_path": "in.rs" } }),
    );
    assert!(!err, "{text}");
    assert!(text.contains("\"tools_called\": 1"), "{text}");

    let mut c = Client::spawn(env!("CARGO_BIN_EXE_codegen-server"), root.path());
    c.initialize();
    let rel = "../".repeat(20) + "escape.txt";
    for (tool, args) in [
        ("file_create", json!({ "path": abs, "content": "pwned" })),
        ("file_create", json!({ "path": rel, "content": "pwned" })),
        (
            "str_replace",
            json!({ "file": abs, "search": "top", "replace": "pwned" }),
        ),
    ] {
        let (err, text) = c.call(tool, args.clone());
        assert!(err, "{tool} {args}: {text}");
    }
    assert_eq!(std::fs::read_to_string(&secret).unwrap(), "top secret");
}

/// bash_tool は allowlist が無ければ使えない
#[test]
fn bash_tool_is_disabled_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = Client::spawn(env!("CARGO_BIN_EXE_codegen-server"), dir.path());
    c.initialize();
    let (err, text) = c.call("bash_tool", json!({ "command": "echo hi" }));
    assert!(err, "{text}");
    assert!(text.contains("SUBAGENT_CODEGEN_ALLOWED_COMMANDS"), "{text}");
    let mut c = Client::spawn(env!("CARGO_BIN_EXE_review-server"), dir.path());
    c.initialize();
    let (err, text) = c.call("bash_tool", json!({ "command": "echo 1 error here" }));
    assert!(
        err && text.contains("SUBAGENT_CODEGEN_ALLOWED_COMMANDS"),
        "{text}"
    );
}

#[cfg(windows)]
const ENV_DUMP: (&str, &str) = ("cmd", "cmd /c set");
#[cfg(not(windows))]
const ENV_DUMP: (&str, &str) = ("env", "env");

/// 子プロセスに API 鍵・Notion token が渡らない。shell の構文も通らない
#[test]
fn bash_tool_child_env_has_no_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::path::absolute(dir.path()).unwrap();
    let root = root.to_string_lossy().into_owned();
    let mut c = Client::spawn_with(
        env!("CARGO_BIN_EXE_codegen-server"),
        dir.path(),
        &[
            FULL,
            ("SUBAGENT_ROOTS", &root),
            ("SUBAGENT_CODEGEN_ALLOWED_COMMANDS", ENV_DUMP.0),
            // 偽の値（本物の鍵ではない）。server には渡すが、bash_tool の子に渡らないことを確かめる
            ("ANTHROPIC_API_KEY", "sk-ant-leak-1"),
            ("SUBAGENT_ANTHROPIC_API_KEY", "sk-ant-leak-2"),
            ("NOTION_TOKEN", "ntn-leak-3"),
        ],
        Stdio::null(),
    );
    c.initialize();
    let (err, text) = c.call("bash_tool", json!({ "command": ENV_DUMP.1 }));
    assert!(!err, "{text}");
    let v: Value = serde_json::from_str(&text).unwrap();
    let stdout = v["stdout"].as_str().unwrap();
    assert!(stdout.to_uppercase().contains("PATH="), "{stdout}");
    for leak in [
        "sk-ant-leak-1",
        "sk-ant-leak-2",
        "ntn-leak-3",
        "ANTHROPIC_API_KEY=",
        "NOTION_TOKEN=",
    ] {
        assert!(!stdout.contains(leak), "{leak} reached the child: {stdout}");
    }
    // 許可外の program と shell の構文は拒否
    let (err, text) = c.call("bash_tool", json!({ "command": "rustc --version" }));
    assert!(err, "{text}");
    let (err, text) = c.call(
        "bash_tool",
        json!({ "command": format!("{} && echo pwned", ENV_DUMP.1) }),
    );
    assert!(err, "{text}");
}

/// direct tool の呼び出しも 1 回ずつ別タスクとして数える（SUBAGENT_MAX_FILES を超えて作れる）
#[test]
fn direct_tool_calls_count_files_per_call() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::path::absolute(dir.path()).unwrap();
    let root = root.to_string_lossy().into_owned();
    let mut c = Client::spawn_with(
        env!("CARGO_BIN_EXE_codegen-server"),
        dir.path(),
        &[FULL, ("SUBAGENT_ROOTS", &root), ("SUBAGENT_MAX_FILES", "2")],
        Stdio::null(),
    );
    c.initialize();
    for i in 0..5 {
        let (err, text) = c.call(
            "file_create",
            json!({ "path": format!("f{i}.txt"), "content": "x" }),
        );
        assert!(!err, "call {i}: {text}");
    }
}
