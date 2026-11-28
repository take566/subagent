//! MCP サーバーの出力を、移植元 TS 版の出力（tests/golden/*.json）と突き合わせる。
//!
//! golden は TS 版（@modelcontextprotocol/sdk 0.5.0）の各サーバーに
//! initialize → tools/list → tools/call を送って記録したもの。
//! ただし `document.json` の tools のうち `notion_page_create` / `notion_page_update` は記録後に
//! 手で足したもの（記録時の TS には無く、TS #45 の name / description / 引数定義に合わせた）。
//! 実行ごとに変わる値（duration_ms、作業ディレクトリ、タイムスタンプ）は正規化して比較する。

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

struct Client {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    non_json_lines: Vec<String>,
}

impl Client {
    fn spawn(bin: &str, cwd: &Path) -> Client {
        Self::spawn_with(bin, cwd, &[], Stdio::null())
    }

    fn spawn_with(bin: &str, cwd: &Path, envs: &[(&str, &str)], stderr: Stdio) -> Client {
        let mut child = Command::new(bin)
            .current_dir(cwd)
            .env("SUBAGENT_LOG_LEVEL", "debug") // ログが stdout を汚さないことも確かめる
            .env_remove("AGENT_ID")
            .env_remove("MAX_CONCURRENT")
            .envs(envs.iter().copied())
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
        }
    }

    fn send(&mut self, msg: Value) {
        writeln!(self.stdin, "{msg}").unwrap();
        self.stdin.flush().unwrap();
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
                Ok(_) => {} // 通知など
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
            Value::String(re.replace_all(&s, "generated-<TIMESTAMP>").into_owned())
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

fn check_server(agent: &str, bin: &str) {
    let golden: Value = serde_json::from_str(
        &std::fs::read_to_string(format!(
            "{}/tests/golden/{agent}.json",
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
    let init = c.request(
        "initialize",
        json!({ "protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": { "name": "golden", "version": "0" } }),
    );
    c.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    assert_eq!(
        init["serverInfo"]["name"], golden["serverInfo"]["name"],
        "{agent}: serverInfo.name"
    );
    assert_eq!(
        init["serverInfo"]["version"], golden["serverInfo"]["version"],
        "{agent}: serverInfo.version"
    );

    // tools/list: name / description / inputSchema が TS 版と一致し、順序も同じ
    let list = c.request("tools/list", json!({}));
    let pick = |tools: &Value| -> Vec<Value> {
        tools
            .as_array()
            .unwrap()
            .iter()
            .map(|t| json!({ "name": t["name"], "description": t["description"], "inputSchema": t["inputSchema"] }))
            .collect()
    };
    assert_eq!(
        pick(&list["tools"]),
        pick(&golden["tools"]),
        "{agent}: tools/list"
    );

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
    check_server("research", env!("CARGO_BIN_EXE_research-server"));
}

#[test]
fn codegen_server_matches_ts() {
    check_server("codegen", env!("CARGO_BIN_EXE_codegen-server"));
}

#[test]
fn review_server_matches_ts() {
    check_server("review", env!("CARGO_BIN_EXE_review-server"));
}

#[test]
fn document_server_matches_ts() {
    check_server("document", env!("CARGO_BIN_EXE_document-server"));
}

/// TS 版でも isError になる入力（golden 記録後に追加した確認）
#[test]
fn invalid_inputs_are_tool_errors() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = Client::spawn(env!("CARGO_BIN_EXE_review-server"), dir.path());
    c.request(
        "initialize",
        json!({ "protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": { "name": "t", "version": "0" } }),
    );
    c.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
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
            &[("AGENT_ID", " research-xyz "), ("MAX_CONCURRENT", "1")],
            Stdio::from(log),
        );
        c.request(
            "initialize",
            json!({ "protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": { "name": "t", "version": "0" } }),
        );
        c.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
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
