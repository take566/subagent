//! codegen-server を replay backend（記録した応答）で動かし、typed tool `codegen` の応答の形と
//! workspace に書いたファイルを確かめる（feature `replay` の中だけ。通信しない）

#![cfg(feature = "replay")]

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{Value, json};

const PROXY_TRAP: &str = "http://127.0.0.1:9";
/// 応答 1 つを待つ上限（これを過ぎたら test を失敗させる。止まったままにしない）
const RECV_TIMEOUT: Duration = Duration::from_secs(30);

/// fixture で codegen-server を起動して `arguments` で codegen を 1 回呼び、(tools/call の result, stderr)
fn call_codegen(root: &Path, fixture: &str, arguments: Value) -> (Value, String) {
    let fixture = format!(
        "{}/tests/golden/replay/{fixture}",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_codegen-server"));
    cmd.current_dir(root);
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
    cmd.env("SUBAGENT_ROOTS", root)
        .env("SUBAGENT_LLM_BACKEND", "replay")
        .env("SUBAGENT_LLM_REPLAY_FILE", &fixture)
        .env("MAX_CONCURRENT", "1");
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn codegen-server");
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    // 行は thread で読み、recv_timeout で待つ（server が止まっても test は止まらない）
    let (tx, rx) = mpsc::channel::<Value>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if let Ok(v) = serde_json::from_str::<Value>(&line)
                && tx.send(v).is_err()
            {
                break;
            }
        }
    });
    let err_thread = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    let mut send = |v: Value| {
        writeln!(stdin, "{v}").unwrap();
        stdin.flush().unwrap();
    };
    let recv = |id: u64| -> Result<Value, String> {
        loop {
            let v = rx
                .recv_timeout(RECV_TIMEOUT)
                .map_err(|e| format!("no response {id} within {RECV_TIMEOUT:?}: {e}"))?;
            if v.get("id") == Some(&json!(id)) {
                return Ok(v);
            }
        }
    };
    send(
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2024-11-05", "capabilities": {},
        "clientInfo": {"name": "test", "version": "0"}}}),
    );
    let result = recv(1).and_then(|_| {
        send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        send(
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
            "name": "codegen", "arguments": arguments}}),
        );
        recv(2)
    });
    // 失敗しても子 process は残さない
    let _ = child.kill();
    let _ = child.wait();
    let stderr = err_thread.join().unwrap_or_default();
    let r = result.unwrap_or_else(|e| panic!("{e}\nstderr:\n{stderr}"));
    (r["result"].clone(), stderr)
}

#[test]
fn replay_codegen_writes_files_and_reports_them() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::path::absolute(dir.path()).unwrap();
    let (result, _) = call_codegen(&root, "codegen.json", json!({"task": "add two numbers"}));
    assert!(result.get("isError").is_none_or(|v| v == false), "{result}");
    let text = result["content"][0]["text"].as_str().unwrap();
    let v: Value = serde_json::from_str(text).unwrap();
    assert_eq!(
        v,
        json!({
            "files": [{"path": "src/add.rs", "kind": "created"}],
            "test_files": [{"path": "tests/add_test.rs", "kind": "created"}],
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

/// 途中で止まったら isError + structuredContent（途中の結果）。message とログには
/// stop_reason と件数だけを載せ、モデルの本文は stderr に出さない
#[test]
fn replay_codegen_incomplete_is_structured_and_not_logged() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::path::absolute(dir.path()).unwrap();
    let (result, stderr) = call_codegen(&root, "codegen_refusal.json", json!({"task": "t"}));
    assert_eq!(result["isError"], true, "{result}");
    assert_eq!(
        result["content"][0]["text"],
        "Error: codegen did not finish (stop_reason: refusal, 1 file changed)"
    );
    let partial = &result["structuredContent"];
    assert_eq!(partial["stop_reason"], "refusal", "{result}");
    assert_eq!(
        partial["files"],
        json!([{"path": "half.txt", "kind": "created"}])
    );
    assert!(
        partial["summary"]
            .as_str()
            .unwrap()
            .contains("SECRET-REFUSAL-TEXT")
    );
    assert!(stderr.contains("Tool execution failed"), "{stderr}");
    assert!(!stderr.contains("SECRET"), "{stderr}");
    assert!(root.join("half.txt").exists());
}
