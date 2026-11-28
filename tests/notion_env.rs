//! `NOTION_TOKEN` は呼び出し時に環境変数から読む（TS #45 と同じ）。
//! 環境変数を書き換えるので、他のテストと並列にならないよう専用のテストバイナリに置き、
//! テストも 1 本にまとめる（同じバイナリ内のテストはスレッドで並列に走るため）。

use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use subagent::Tool;
use subagent::tools::notion::{
    NotionClient, NotionHttp, NotionHttpRequest, NotionHttpResponse, NotionPageCreateTool,
    token_from_env,
};

#[derive(Default)]
struct RecordingHttp {
    calls: Mutex<Vec<NotionHttpRequest>>,
}

#[async_trait]
impl NotionHttp for RecordingHttp {
    async fn send(&self, req: NotionHttpRequest) -> subagent::Result<NotionHttpResponse> {
        self.calls.lock().unwrap().push(req);
        Ok(NotionHttpResponse {
            status: 200,
            status_text: "OK".into(),
            body: json!({ "id": "pid", "url": "u" }).to_string(),
        })
    }
}

fn auth_header(req: &NotionHttpRequest) -> &str {
    &req.headers
        .iter()
        .find(|(k, _)| k == "Authorization")
        .unwrap()
        .1
}

fn params() -> subagent::JsonMap {
    match json!({ "parent_page_id": "p", "title": "t" }) {
        Value::Object(m) => m,
        _ => unreachable!(),
    }
}

fn set(v: Option<&str>) {
    // SAFETY: このテストバイナリにはこの 1 本のテストしか無く、他のスレッドは環境変数を読まない
    unsafe {
        match v {
            Some(v) => std::env::set_var("NOTION_TOKEN", v),
            None => std::env::remove_var("NOTION_TOKEN"),
        }
    }
}

#[tokio::test]
async fn token_is_read_from_env_at_call_time() {
    set(None);
    assert_eq!(token_from_env(), None);
    set(Some(""));
    assert_eq!(token_from_env(), None, "空文字列は未設定扱い");
    set(Some("secret_x"));
    assert_eq!(token_from_env().as_deref(), Some("secret_x"));

    // クライアントを作った「後」に環境変数を変えても、呼び出し時の値が使われる
    set(None);
    let http = Arc::new(RecordingHttp::default());
    let tool = NotionPageCreateTool::new(NotionClient::from_env_with(http.clone()));

    let e = tool.invoke(params()).await.unwrap_err();
    assert!(e.to_string().contains("NOTION_TOKEN"), "{e}");
    assert!(http.calls.lock().unwrap().is_empty());

    set(Some("first"));
    tool.invoke(params()).await.unwrap();
    set(Some("second"));
    tool.invoke(params()).await.unwrap();

    let calls = http.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(auth_header(&calls[0]), "Bearer first");
    assert_eq!(auth_header(&calls[1]), "Bearer second");
    drop(calls);
    set(None);
}
