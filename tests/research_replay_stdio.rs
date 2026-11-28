//! research-server を replay backend（tests/golden/replay/research.json）で子プロセスとして起動し、
//! typed tool の応答の形を確かめる（feature `replay` の時だけ）。
//!
//! 子プロセスには API 鍵・`SUBAGENT_*` / `ANTHROPIC_*` の env を渡さず、proxy を何も listen していない
//! trap に向ける（外へは通信しない）。
#![cfg(feature = "replay")]

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{Value, json};

const PROXY_TRAP: &str = "http://127.0.0.1:9";

#[test]
fn research_server_replays_the_fixture_with_untrusted_marking() {
    let fixture = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/golden/replay/research.json"
    );
    let dir = tempfile::tempdir().unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_research-server"));
    cmd.current_dir(dir.path());
    for (k, _) in std::env::vars_os() {
        let k = k.to_string_lossy();
        if k.starts_with("SUBAGENT_")
            || k.starts_with("ANTHROPIC_")
            || k.eq_ignore_ascii_case("no_proxy")
            || k == "MAX_CONCURRENT"
            || k == "AGENT_ID"
        {
            cmd.env_remove(k.as_ref());
        }
    }
    for k in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        cmd.env(k, PROXY_TRAP);
    }
    cmd.env("SUBAGENT_LLM_BACKEND", "replay")
        .env("SUBAGENT_LLM_REPLAY_FILE", fixture)
        .env("MAX_CONCURRENT", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = cmd.spawn().expect("spawn research-server");
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();

    // 応答を別 thread で読み、待ちすぎたら失敗にする（server が詰まっても test を止めない）
    let (tx, rx) = mpsc::channel::<Value>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if let Ok(v) = serde_json::from_str::<Value>(line.trim())
                && tx.send(v).is_err()
            {
                break;
            }
        }
    });
    let mut send = |msg: Value| {
        writeln!(stdin, "{msg}").unwrap();
        stdin.flush().unwrap();
    };
    let wait_for = |id: u64| -> Value {
        loop {
            let msg = rx
                .recv_timeout(Duration::from_secs(30))
                .expect("response from research-server");
            if msg.get("id") == Some(&json!(id)) {
                assert!(msg.get("error").is_none(), "{msg}");
                return msg["result"].clone();
            }
        }
    };

    send(
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}
        }}),
    );
    wait_for(1);
    send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    send(
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
            "name": "research", "arguments": {"query": "rust async", "depth": "quick"}
        }}),
    );
    let r = wait_for(2);
    let _ = child.kill();
    let _ = child.wait();

    assert!(r.get("isError").is_none_or(|v| v == false), "{r}");
    let text = r["content"][0]["text"].as_str().unwrap();
    assert!(!text.contains("cited_text"), "{text}");
    let v: Value = serde_json::from_str(text).unwrap();
    assert!(
        v["untrusted_notice"]
            .as_str()
            .unwrap()
            .contains("untrusted")
    );
    assert!(
        v["text"]
            .as_str()
            .unwrap()
            .starts_with("<untrusted_research_result>")
    );
    assert_eq!(v["grounded"], true);
    assert_eq!(v["partial"], false);
    assert_eq!(v["stop_reason"], "end_turn");
    assert_eq!(v["sources"].as_array().unwrap().len(), 2);
    assert_eq!(v["tokens_used"], 5150);
}
