//! Notion ページ作成/更新ツール（TS `src/agents/document/document-agent.ts` #45 の移植）
//!
//! トークンは環境変数 `NOTION_TOKEN` から**呼び出し時に**読む（TS と同じ）。
//! HTTP は [`NotionHttp`] トレイト越しに送るので、テストでは差し替えて実 API を叩かない。

use crate::core::tool::{Tool, param_defs};
use crate::error::{Error, Result};
use crate::types::JsonMap;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;

pub const NOTION_API: &str = "https://api.notion.com/v1";
pub const NOTION_VERSION: &str = "2022-06-28";

/// Notion API へのリクエスト
#[derive(Debug, Clone, PartialEq)]
pub struct NotionHttpRequest {
    pub method: &'static str,
    pub url: String,
    /// `(name, value)`。順序は TS の `headers` と同じ
    pub headers: Vec<(String, String)>,
    pub body: Value,
}

/// Notion API からのレスポンス
#[derive(Debug, Clone, PartialEq)]
pub struct NotionHttpResponse {
    pub status: u16,
    pub status_text: String,
    pub body: String,
}

/// HTTP 送信部分（テストで差し替える）
#[async_trait]
pub trait NotionHttp: Send + Sync {
    async fn send(&self, req: NotionHttpRequest) -> Result<NotionHttpResponse>;
}

/// トークンの取得元
pub type TokenSource = Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// 環境変数 `NOTION_TOKEN` を読む（空文字列は未設定扱い。TS の `if (!token)` と同じ）
pub fn token_from_env() -> Option<String> {
    std::env::var("NOTION_TOKEN").ok().filter(|t| !t.is_empty())
}

/// Notion API クライアント（ツール間で共有する）
#[derive(Clone)]
pub struct NotionClient {
    http: Arc<dyn NotionHttp>,
    token: TokenSource,
}

impl NotionClient {
    pub fn new(http: Arc<dyn NotionHttp>, token: TokenSource) -> Self {
        NotionClient { http, token }
    }

    /// 実際の Notion API（reqwest）と環境変数 `NOTION_TOKEN` を使うクライアント
    pub fn from_env() -> Self {
        NotionClient::new(Arc::new(ReqwestHttp::default()), Arc::new(token_from_env))
    }

    /// Notion API 呼び出し。トークンは呼び出しのたびに読む
    async fn request(&self, method: &'static str, endpoint: &str, body: Value) -> Result<Value> {
        let token = (self.token)()
            .ok_or_else(|| Error::other("NOTION_TOKEN environment variable is not set"))?;
        let res = self
            .http
            .send(NotionHttpRequest {
                method,
                url: format!("{NOTION_API}{endpoint}"),
                headers: vec![
                    ("Authorization".into(), format!("Bearer {token}")),
                    ("Notion-Version".into(), NOTION_VERSION.into()),
                    ("Content-Type".into(), "application/json".into()),
                ],
                body,
            })
            .await?;
        // fetch の `res.ok` と同じく 2xx のみ成功
        if !(200..300).contains(&res.status) {
            return Err(Error::other(format!(
                "Notion API error: {} {}",
                res.status, res.status_text
            )));
        }
        Ok(serde_json::from_str(&res.body)?)
    }
}

impl Default for NotionClient {
    fn default() -> Self {
        Self::from_env()
    }
}

/// reqwest（rustls）で送る [`NotionHttp`]。Client は最初の送信時に作る
#[derive(Default)]
pub struct ReqwestHttp {
    client: std::sync::OnceLock<reqwest::Client>,
}

#[async_trait]
impl NotionHttp for ReqwestHttp {
    async fn send(&self, req: NotionHttpRequest) -> Result<NotionHttpResponse> {
        let client = self.client.get_or_init(reqwest::Client::new);
        let method = reqwest::Method::from_bytes(req.method.as_bytes())
            .map_err(|e| Error::other(e.to_string()))?;
        let mut builder = client.request(method, &req.url);
        for (k, v) in &req.headers {
            builder = builder.header(k, v);
        }
        let res = builder
            .body(req.body.to_string())
            .send()
            .await
            .map_err(|e| Error::other(format!("Notion request failed: {e}")))?;
        let status = res.status();
        let body = res
            .text()
            .await
            .map_err(|e| Error::other(format!("Notion response read failed: {e}")))?;
        Ok(NotionHttpResponse {
            status: status.as_u16(),
            status_text: status.canonical_reason().unwrap_or_default().to_string(),
            body,
        })
    }
}

/// 文字列パラメータ。未指定と null は None、文字列以外はエラー
fn opt_string<'a>(params: &'a JsonMap, key: &str) -> Result<Option<&'a str>> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(Error::InvalidParameter(format!("`{key}` must be a string"))),
    }
}

/// 空でない文字列パラメータ（TS の truthy 判定相当）
fn non_empty<'a>(params: &'a JsonMap, key: &str) -> Result<Option<&'a str>> {
    Ok(opt_string(params, key)?.filter(|s| !s.is_empty()))
}

fn title_property(title: &str) -> Value {
    json!({ "title": { "title": [{ "type": "text", "text": { "content": title } }] } })
}

/// Notion の rich_text 1 要素の上限（文字数。JS の length と同じく UTF-16 単位）
const RICH_TEXT_LIMIT: usize = 2000;
/// 1 回の children 追加の上限ブロック数
const BLOCKS_PER_REQUEST: usize = 100;

/// テキストを Notion の paragraph ブロック列に変換（空行は飛ばし、長い行は分割する）
fn to_paragraph_blocks(content: &str) -> Vec<Value> {
    content
        .split('\n')
        .filter(|line| !line.trim().is_empty())
        .flat_map(|line| split_utf16(line, RICH_TEXT_LIMIT))
        .map(|chunk| {
            json!({
                "object": "block",
                "type": "paragraph",
                "paragraph": { "rich_text": [{ "type": "text", "text": { "content": chunk } }] },
            })
        })
        .collect()
}

/// Notion ページ作成ツール
pub struct NotionPageCreateTool {
    params: JsonMap,
    client: NotionClient,
}

impl NotionPageCreateTool {
    pub fn new(client: NotionClient) -> Self {
        NotionPageCreateTool {
            params: param_defs(&[
                ("parent_page_id", "string", true),
                ("title", "string", true),
                ("content", "string", false),
            ]),
            client,
        }
    }
}

#[async_trait]
impl Tool for NotionPageCreateTool {
    fn name(&self) -> &str {
        "notion_page_create"
    }
    fn description(&self) -> &str {
        "Notionページを作成"
    }
    fn parameters(&self) -> &JsonMap {
        &self.params
    }

    async fn invoke(&self, params: JsonMap) -> Result<Value> {
        let parent = non_empty(&params, "parent_page_id")?;
        let title = non_empty(&params, "title")?;
        let content = opt_string(&params, "content")?.unwrap_or("");
        let (Some(parent), Some(title)) = (parent, title) else {
            return Err(Error::InvalidParameter(
                "parent_page_id and title are required".into(),
            ));
        };
        // 作成時の children は 100 ブロックまで。残りは追記する
        let blocks = to_paragraph_blocks(content);
        let first: Vec<Value> = blocks.iter().take(BLOCKS_PER_REQUEST).cloned().collect();
        let page = self
            .client
            .request(
                "POST",
                "/pages",
                json!({
                    "parent": { "page_id": parent },
                    "properties": title_property(title),
                    "children": first,
                }),
            )
            .await?;
        let page_id = page.get("id").cloned();
        if blocks.len() > BLOCKS_PER_REQUEST {
            let id = encode_uri_component(&crate::tools::js_display(page_id.as_ref()));
            for chunk in blocks[BLOCKS_PER_REQUEST..].chunks(BLOCKS_PER_REQUEST) {
                self.client
                    .request(
                        "PATCH",
                        &format!("/blocks/{id}/children"),
                        json!({ "children": chunk }),
                    )
                    .await?;
            }
        }
        let mut out = JsonMap::new();
        if let Some(id) = page_id {
            out.insert("page_id".into(), id);
        }
        if let Some(url) = page.get("url") {
            out.insert("url".into(), url.clone());
        }
        out.insert("created".into(), json!(true));
        Ok(Value::Object(out))
    }
}

/// Notion ページ更新ツール（タイトル変更と本文ブロックの追記）
pub struct NotionPageUpdateTool {
    params: JsonMap,
    client: NotionClient,
}

impl NotionPageUpdateTool {
    pub fn new(client: NotionClient) -> Self {
        NotionPageUpdateTool {
            params: param_defs(&[
                ("page_id", "string", true),
                ("title", "string", false),
                ("content", "string", false),
            ]),
            client,
        }
    }
}

#[async_trait]
impl Tool for NotionPageUpdateTool {
    fn name(&self) -> &str {
        "notion_page_update"
    }
    fn description(&self) -> &str {
        "Notionページを更新（タイトル変更・本文追記）"
    }
    fn parameters(&self) -> &JsonMap {
        &self.params
    }

    async fn invoke(&self, params: JsonMap) -> Result<Value> {
        let Some(page_id) = non_empty(&params, "page_id")? else {
            return Err(Error::InvalidParameter("page_id is required".into()));
        };
        let title = non_empty(&params, "title")?;
        let content = non_empty(&params, "content")?;
        if title.is_none() && content.is_none() {
            return Err(Error::InvalidParameter(
                "title or content is required".into(),
            ));
        }
        let id = encode_uri_component(page_id);
        if let Some(title) = title {
            self.client
                .request(
                    "PATCH",
                    &format!("/pages/{id}"),
                    json!({ "properties": title_property(title) }),
                )
                .await?;
        }
        if let Some(content) = content {
            let blocks = to_paragraph_blocks(content);
            for chunk in blocks.chunks(BLOCKS_PER_REQUEST) {
                self.client
                    .request(
                        "PATCH",
                        &format!("/blocks/{id}/children"),
                        json!({ "children": chunk }),
                    )
                    .await?;
            }
        }
        Ok(json!({ "page_id": page_id, "updated": true }))
    }
}

/// `max` UTF-16 単位ごとに分割する（JS の `slice` と同じ単位。ただしサロゲートペアは切らない）
fn split_utf16(line: &str, max: usize) -> Vec<String> {
    let mut chunks = vec![];
    let mut cur = String::new();
    let mut len = 0;
    for ch in line.chars() {
        if len + ch.len_utf16() > max && !cur.is_empty() {
            chunks.push(std::mem::take(&mut cur));
            len = 0;
        }
        cur.push(ch);
        len += ch.len_utf16();
    }
    if !cur.is_empty() {
        chunks.push(cur);
    }
    chunks
}

/// JS の `encodeURIComponent` 相当
fn encode_uri_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// 送られたリクエストを記録し、決まったレスポンスを返す
    struct MockHttp {
        calls: Mutex<Vec<NotionHttpRequest>>,
        status: u16,
        body: String,
    }

    impl MockHttp {
        fn ok(body: Value) -> Arc<Self> {
            Arc::new(MockHttp {
                calls: Mutex::new(vec![]),
                status: 200,
                body: body.to_string(),
            })
        }
        fn calls(&self) -> Vec<NotionHttpRequest> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl NotionHttp for MockHttp {
        async fn send(&self, req: NotionHttpRequest) -> Result<NotionHttpResponse> {
            self.calls.lock().unwrap().push(req);
            Ok(NotionHttpResponse {
                status: self.status,
                status_text: if self.status == 200 {
                    "OK"
                } else {
                    "Unauthorized"
                }
                .into(),
                body: self.body.clone(),
            })
        }
    }

    fn client(http: &Arc<MockHttp>, token: Option<&str>) -> NotionClient {
        let token = token.map(str::to_string);
        NotionClient::new(http.clone(), Arc::new(move || token.clone()))
    }

    fn map(v: Value) -> JsonMap {
        v.as_object().unwrap().clone()
    }

    fn pid_http() -> Arc<MockHttp> {
        MockHttp::ok(json!({ "id": "pid", "url": "u" }))
    }

    #[tokio::test]
    async fn errors_without_token_and_does_not_call_http() {
        let http = pid_http();
        let e = NotionPageCreateTool::new(client(&http, None))
            .invoke(map(json!({ "parent_page_id": "p", "title": "t" })))
            .await
            .unwrap_err();
        assert!(e.to_string().contains("NOTION_TOKEN"), "{e}");
        assert!(http.calls().is_empty());
    }

    #[tokio::test]
    async fn rejects_empty_title_parent_content() {
        let http = pid_http();
        let c = client(&http, Some("test-token"));
        assert!(
            NotionPageCreateTool::new(c.clone())
                .invoke(map(json!({ "parent_page_id": "p", "title": "" })))
                .await
                .is_err()
        );
        assert!(
            NotionPageCreateTool::new(c.clone())
                .invoke(map(json!({ "title": "t" })))
                .await
                .is_err()
        );
        assert!(
            NotionPageUpdateTool::new(c.clone())
                .invoke(map(json!({ "page_id": "p" })))
                .await
                .is_err()
        );
        assert!(
            NotionPageUpdateTool::new(c)
                .invoke(map(json!({ "title": "t" })))
                .await
                .is_err()
        );
        assert!(http.calls().is_empty());
    }

    #[tokio::test]
    async fn create_sends_page_request_with_headers() {
        let http = pid_http();
        let r = NotionPageCreateTool::new(client(&http, Some("test-token")))
            .invoke(map(
                json!({ "parent_page_id": "p", "title": "t", "content": "a\n\n  \nb" }),
            ))
            .await
            .unwrap();
        assert_eq!(r, json!({ "page_id": "pid", "url": "u", "created": true }));
        let calls = http.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].method, "POST");
        assert_eq!(calls[0].url, "https://api.notion.com/v1/pages");
        assert_eq!(
            calls[0].headers,
            vec![
                ("Authorization".to_string(), "Bearer test-token".to_string()),
                ("Notion-Version".to_string(), "2022-06-28".to_string()),
                ("Content-Type".to_string(), "application/json".to_string()),
            ]
        );
        let para = |s: &str| {
            json!({
                "object": "block",
                "type": "paragraph",
                "paragraph": { "rich_text": [{ "type": "text", "text": { "content": s } }] },
            })
        };
        // 空行（空白だけの行を含む）はブロックにしない
        assert_eq!(
            calls[0].body,
            json!({
                "parent": { "page_id": "p" },
                "properties": { "title": { "title": [{ "type": "text", "text": { "content": "t" } }] } },
                "children": [para("a"), para("b")],
            })
        );
    }

    #[tokio::test]
    async fn create_appends_blocks_beyond_100() {
        let http = pid_http();
        let content = (0..250)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        NotionPageCreateTool::new(client(&http, Some("test-token")))
            .invoke(map(
                json!({ "parent_page_id": "p", "title": "t", "content": content }),
            ))
            .await
            .unwrap();
        let calls = http.calls();
        assert_eq!(calls.len(), 3);
        let sizes: Vec<usize> = calls
            .iter()
            .map(|c| c.body["children"].as_array().unwrap().len())
            .collect();
        assert_eq!(sizes, [100, 100, 50]);
        assert!(calls[1].url.contains("/blocks/pid/children"));
        assert_eq!(calls[1].method, "PATCH");
        assert_eq!(
            calls[2].body["children"][0]["paragraph"]["rich_text"][0]["text"]["content"],
            "line 200"
        );
    }

    #[tokio::test]
    async fn splits_lines_longer_than_2000_chars() {
        let http = pid_http();
        NotionPageCreateTool::new(client(&http, Some("test-token")))
            .invoke(map(
                json!({ "parent_page_id": "p", "title": "t", "content": "a".repeat(4500) }),
            ))
            .await
            .unwrap();
        let children = http.calls()[0].body["children"].clone();
        let lens: Vec<usize> = children
            .as_array()
            .unwrap()
            .iter()
            .map(|b| {
                b["paragraph"]["rich_text"][0]["text"]["content"]
                    .as_str()
                    .unwrap()
                    .len()
            })
            .collect();
        assert_eq!(lens, [2000, 2000, 500]);
    }

    #[test]
    fn chunks_count_utf16_units_without_splitting_chars() {
        // 2000 は JS の length（UTF-16 単位）。サロゲートペアは途中で切らない
        let line = format!("{}{}", "a".repeat(1999), "😀b");
        let chunks = split_utf16(&line, 2000);
        assert_eq!(chunks, vec!["a".repeat(1999), "😀b".to_string()]);
        let jp = "日".repeat(2001);
        let chunks = split_utf16(&jp, 2000);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[1], "日");
    }

    #[tokio::test]
    async fn update_patches_title_and_appends_content() {
        let http = pid_http();
        let r = NotionPageUpdateTool::new(client(&http, Some("test-token")))
            .invoke(map(
                json!({ "page_id": "a/b c", "title": "New", "content": "x" }),
            ))
            .await
            .unwrap();
        assert_eq!(r, json!({ "page_id": "a/b c", "updated": true }));
        let calls = http.calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].method, "PATCH");
        // page_id は encodeURIComponent 相当でエスケープする
        assert_eq!(calls[0].url, "https://api.notion.com/v1/pages/a%2Fb%20c");
        assert_eq!(
            calls[0].body,
            json!({ "properties": { "title": { "title": [{ "type": "text", "text": { "content": "New" } }] } } })
        );
        assert_eq!(
            calls[1].url,
            "https://api.notion.com/v1/blocks/a%2Fb%20c/children"
        );
        assert_eq!(calls[1].body["children"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn update_content_only_skips_title_patch() {
        let http = pid_http();
        let content = (0..150)
            .map(|i| format!("l{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        NotionPageUpdateTool::new(client(&http, Some("test-token")))
            .invoke(map(json!({ "page_id": "p", "content": content })))
            .await
            .unwrap();
        let calls = http.calls();
        assert_eq!(calls.len(), 2);
        assert!(calls.iter().all(|c| c.url.ends_with("/blocks/p/children")));
    }

    #[tokio::test]
    async fn non_ok_response_is_error() {
        let http = Arc::new(MockHttp {
            calls: Mutex::new(vec![]),
            status: 401,
            body: "{}".into(),
        });
        let e = NotionPageUpdateTool::new(client(&http, Some("bad")))
            .invoke(map(json!({ "page_id": "p", "title": "t" })))
            .await
            .unwrap_err();
        assert_eq!(e.to_string(), "Notion API error: 401 Unauthorized");
    }

    #[test]
    fn encode_uri_component_matches_js() {
        assert_eq!(
            encode_uri_component("aZ09-_.!~*'() /?#&=日"),
            "aZ09-_.!~*'()%20%2F%3F%23%26%3D%E6%97%A5"
        );
    }
}
