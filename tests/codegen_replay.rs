//! codegen-server を replay backend（記録した応答）で動かし、typed tool `codegen` の応答の形と
//! workspace に書いたファイルを確かめる（feature `replay` の中だけ。通信しない）

#![cfg(feature = "replay")]

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

const PROXY_TRAP: &str = "http://127.0.0.1:9";

#[test]
fn replay_codegen_writes_files_and_reports_them() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::path::absolute(dir.path()).unwrap();
    let fixture = format!(
        "{}/tests/golden/replay/codegen.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_codegen-server"));
    cmd.current_dir(&root);
    for (k, _) in std::env::vars_os() {
        let k = k.to_string_lossy();
        if k.starts_with("SUBAGENT_") || k.starts_with("ANTHROPIC_") || k == "MAX_CONCURRENT" {
            cmd.env_remove(k.as_ref());
        }
    }
    for k in ["HTTPS_PROXY", "HTTP_PROXY", "https_proxy", "http_proxy"] {
        cmd.env(k, PROXY_TRAP);
    }
    cmd.env_remove("NO_PROXY").env_remove("no_proxy");
    cmd.env("SUBAGENT_ROOTS", &root)
        .env("SUBAGENT_LLM_BACKEND", "replay")
        .env("SUBAGENT_LLM_REPLAY_FILE", &fixture)
        .env("MAX_CONCURRENT", "1");
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn codegen-server");
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut send = |v: Value| {
        writeln!(stdin, "{v}").unwrap();
        stdin.flush().unwrap();
    };
    let mut recv = |id: u64| -> Value {
        loop {
            let mut line = String::new();
            assert!(stdout.read_line(&mut line).unwrap() > 0, "server closed");
            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if v.get("id") == Some(&json!(id)) {
                return v;
            }
        }
    };
    send(
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2024-11-05", "capabilities": {},
        "clientInfo": {"name": "test", "version": "0"}}}),
    );
    recv(1);
    send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    send(
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
        "name": "codegen", "arguments": {"task": "add two numbers"}}}),
    );
    let r = recv(2);
    drop(send);
    let _ = child.kill();
    let _ = child.wait();

    let result = &r["result"];
    assert!(result.get("isError").is_none_or(|v| v == false), "{result}");
    let text = result["content"][0]["text"].as_str().unwrap();
    let v: Value = serde_json::from_str(text).unwrap();
    assert_eq!(
        v,
        json!({
            "files": ["src/add.rs"],
            "test_files": ["tests/add_test.rs"],
            "summary": "Added add() and a test.",
            "iterations": 2,
            "tokens_used": 370,
            "stop_reason": "end_turn",
            "notes": []
        })
    );
    assert!(
        std::fs::read_to_string(root.join("src/add.rs"))
            .unwrap()
            .contains("a + b")
    );
    assert!(root.join("tests/add_test.rs").exists());
}
