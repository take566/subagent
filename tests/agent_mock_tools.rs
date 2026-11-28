//! 各 Agent をモックツールで動かす結合テスト（TS `tests/agents/*-agent.test.ts`、#49 の移植）
//!
//! TS 版は `vi.spyOn(agent.getTool(name), 'invoke')` で 1 本だけ差し替えていた。
//! Rust 版は `default_tools()` の中の同名ツールを [`MockTool`] に置き換えて `with_tools` に渡す。

use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use subagent::{
    AgentStatus, Backoff, BehaviorConfig, CodeGenAgent, DocumentAgent, Error, JsonMap,
    ResearchAgent, Result, RetryPolicy, ReviewAgent, Subagent, Task, TaskStatus, Tool,
};

type Handler = Box<dyn Fn(&JsonMap) -> Result<Value> + Send + Sync>;

/// 呼び出し回数と引数を記録するモックツール
struct MockTool {
    name: String,
    params: JsonMap,
    calls: AtomicUsize,
    args: Mutex<Vec<JsonMap>>,
    handler: Handler,
}

impl MockTool {
    fn new(
        name: &str,
        handler: impl Fn(&JsonMap) -> Result<Value> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(MockTool {
            name: name.into(),
            params: JsonMap::new(),
            calls: AtomicUsize::new(0),
            args: Mutex::new(vec![]),
            handler: Box::new(handler),
        })
    }

    fn failing(name: &str, msg: &'static str) -> Arc<Self> {
        Self::new(name, move |_| Err(Error::other(msg)))
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Tool for MockTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        "mock"
    }
    fn parameters(&self) -> &JsonMap {
        &self.params
    }
    async fn invoke(&self, params: JsonMap) -> Result<Value> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.args.lock().unwrap().push(params.clone());
        (self.handler)(&params)
    }
}

/// `tools` のうち `mock` と同名のものを差し替える（無ければ panic）
fn replace(mut tools: Vec<Arc<dyn Tool>>, mock: Arc<MockTool>) -> Vec<Arc<dyn Tool>> {
    let i = tools
        .iter()
        .position(|t| t.name() == mock.name)
        .unwrap_or_else(|| panic!("no tool named {}", mock.name));
    tools[i] = mock;
    tools
}

fn task(id: &str, action: &str, params: Value) -> Task {
    Task::new(id, action, params)
}

fn assert_failed_state(agent: &dyn Subagent, code: Option<&str>) {
    let state = agent.get_state();
    assert_eq!(state.status, AgentStatus::Failed);
    assert!(state.current_task.is_none());
    assert_eq!(state.context.history.len(), 1);
    let h = &state.context.history[0];
    assert_eq!(h.status, TaskStatus::Failed);
    if let Some(code) = code {
        assert_eq!(h.error.as_ref().unwrap().code, code);
    }
}

// ---------------------------------------------------------------------------
// ResearchAgent
// ---------------------------------------------------------------------------

fn research_config() -> subagent::SubagentConfig {
    let mut c = ResearchAgent::default_config();
    c.behavior = BehaviorConfig {
        retry_policy: Some(RetryPolicy {
            max_attempts: 2,
            backoff: Backoff::Fixed,
            initial_delay_ms: Some(1),
        }),
        timeout: Some(1000),
        fallback: Some("return_partial".into()),
    };
    c
}

#[tokio::test]
async fn research_happy_path_search_and_fetch() {
    let search = MockTool::new("web_search", |_| {
        Ok(json!({ "results": [{ "url": "u1" }, { "url": "u2" }] }))
    });
    let fetch = MockTool::new("web_fetch", |p| {
        let url = p["url"].as_str().unwrap();
        Ok(json!({ "url": url, "content": format!("body-{url}") }))
    });
    let tools = replace(
        replace(ResearchAgent::default_tools(), search.clone()),
        fetch.clone(),
    );
    let agent = ResearchAgent::with_tools(research_config(), tools);

    let res = agent
        .execute(task("t1", "research", json!({ "query": "vitest" })))
        .await
        .unwrap();

    assert_eq!(res.status, TaskStatus::Completed);
    assert_eq!(search.calls(), 1);
    assert_eq!(fetch.calls(), 2);
    let result = res.result.unwrap();
    assert!(result["summary"].as_str().unwrap().contains("body-u1"));
    assert_eq!(result["sources"].as_array().unwrap().len(), 2);
    assert_eq!(res.metrics.unwrap().tools_called, Some(3));
    assert_eq!(agent.get_state().status, AgentStatus::Completed);
}

#[tokio::test]
async fn research_failure_retries_then_records_failed() {
    let search = MockTool::failing("web_search", "search down");
    let agent = ResearchAgent::with_tools(
        research_config(),
        replace(ResearchAgent::default_tools(), search.clone()),
    );

    let e = agent
        .execute(task("t1", "research", json!({ "query": "vitest" })))
        .await
        .unwrap_err();

    assert!(e.to_string().contains("search down"), "{e}");
    assert_eq!(search.calls(), 2); // retry_policy.max_attempts
    assert_failed_state(&agent, Some("RESEARCH_ERROR"));
}

// ---------------------------------------------------------------------------
// CodeGenAgent
// ---------------------------------------------------------------------------

#[tokio::test]
async fn codegen_happy_path_creates_source_and_test() {
    let create = MockTool::new("file_create", |p| {
        Ok(json!({ "path": p["path"], "created": true }))
    });
    let agent = CodeGenAgent::with_tools(
        CodeGenAgent::default_config(),
        replace(CodeGenAgent::default_tools(None), create.clone()),
    );

    let res = agent
        .execute(task("t2", "codegen", json!({ "specification": "hello" })))
        .await
        .unwrap();

    assert_eq!(res.status, TaskStatus::Completed);
    assert_eq!(create.calls(), 2);
    let result = res.result.unwrap();
    assert_eq!(result["files"].as_array().unwrap().len(), 1);
    assert_eq!(result["test_files"].as_array().unwrap().len(), 1);
    assert_eq!(agent.get_state().status, AgentStatus::Completed);
}

#[tokio::test]
async fn codegen_failure_records_failed() {
    let create = MockTool::failing("file_create", "disk full");
    let agent = CodeGenAgent::with_tools(
        CodeGenAgent::default_config(),
        replace(CodeGenAgent::default_tools(None), create.clone()),
    );

    let e = agent
        .execute(task("t2", "codegen", json!({ "specification": "hello" })))
        .await
        .unwrap_err();

    assert!(e.to_string().contains("disk full"), "{e}");
    assert_eq!(create.calls(), 1); // CodeGen は retry しない
    assert_failed_state(&agent, Some("CODEGEN_ERROR"));
}

// ---------------------------------------------------------------------------
// ReviewAgent
// ---------------------------------------------------------------------------

#[tokio::test]
async fn review_happy_path_merges_view_and_lint() {
    let view = MockTool::new("view", |_| Ok(json!({ "content": "// ok\nconst a = 1;" })));
    let lint = MockTool::new("bash_tool", |_| {
        Ok(json!({ "issues": [{ "type": "error", "message": "lint-issue" }] }))
    });
    let agent = ReviewAgent::with_tools(
        ReviewAgent::default_config(),
        replace(
            replace(ReviewAgent::default_tools(), view.clone()),
            lint.clone(),
        ),
    );

    // Rust 版は lint_command 指定時だけ lint を実行する（TS は npm run lint 固定）。
    // ファイルは workspace 内の相対 path を source_path で渡す（絶対パスの source_code は読まない）
    let res = agent
        .execute(task(
            "t4",
            "review",
            json!({ "source_path": "src/a.ts", "lint_command": "lint" }),
        ))
        .await
        .unwrap();

    assert_eq!(res.status, TaskStatus::Completed);
    assert_eq!(view.calls(), 1);
    assert_eq!(lint.calls(), 1);
    assert_eq!(lint.args.lock().unwrap()[0]["command"], "lint");
    let result = res.result.unwrap();
    assert_eq!(
        result["issues"],
        json!([{ "type": "error", "message": "lint-issue" }])
    );
    assert!(result["score"].is_number());
}

#[tokio::test]
async fn review_failure_when_view_errors_skips_lint() {
    let view = MockTool::failing("view", "ENOENT");
    let lint = MockTool::new("bash_tool", |_| Ok(json!({ "issues": [] })));
    let agent = ReviewAgent::with_tools(
        ReviewAgent::default_config(),
        replace(
            replace(ReviewAgent::default_tools(), view.clone()),
            lint.clone(),
        ),
    );

    let e = agent
        .execute(task(
            "t4",
            "review",
            json!({ "source_path": "missing.ts", "lint_command": "lint" }),
        ))
        .await
        .unwrap_err();

    assert!(e.to_string().contains("ENOENT"), "{e}");
    assert_eq!(lint.calls(), 0);
    assert_failed_state(&agent, Some("REVIEW_ERROR"));
}

#[tokio::test]
async fn review_lint_error_is_optional() {
    let lint = MockTool::failing("bash_tool", "lint boom");
    let agent = ReviewAgent::with_tools(
        ReviewAgent::default_config(),
        replace(ReviewAgent::default_tools(), lint.clone()),
    );

    let res = agent
        .execute(task(
            "t4",
            "review",
            json!({ "source_code": "const x = 1;", "lint_command": "lint" }),
        ))
        .await
        .unwrap();

    assert_eq!(lint.calls(), 1);
    assert_eq!(res.status, TaskStatus::Completed);
    assert_eq!(res.result.unwrap()["issues"], json!([]));
}

// ---------------------------------------------------------------------------
// DocumentAgent
// ---------------------------------------------------------------------------

#[tokio::test]
async fn document_happy_path_returns_created_path() {
    let create = MockTool::new("file_create", |_| {
        Ok(json!({ "path": "/tmp/doc.md", "created": true }))
    });
    let agent = DocumentAgent::with_tools(
        DocumentAgent::default_config(),
        replace(DocumentAgent::default_tools(None), create.clone()),
    );

    let res = agent
        .execute(task(
            "t3",
            "document",
            json!({ "content": "# Title", "format": "markdown" }),
        ))
        .await
        .unwrap();

    assert_eq!(res.status, TaskStatus::Completed);
    assert_eq!(create.calls(), 1);
    let result = res.result.unwrap();
    assert_eq!(result["document_path"], "/tmp/doc.md");
    assert_eq!(result["format"], "markdown");
    assert_eq!(agent.get_state().status, AgentStatus::Completed);
}

#[tokio::test]
async fn document_failure_records_failed() {
    let create = MockTool::failing("file_create", "EACCES");
    let agent = DocumentAgent::with_tools(
        DocumentAgent::default_config(),
        replace(DocumentAgent::default_tools(None), create.clone()),
    );

    let e = agent
        .execute(task(
            "t3",
            "document",
            json!({ "content": "# Title", "format": "markdown" }),
        ))
        .await
        .unwrap_err();

    assert!(e.to_string().contains("EACCES"), "{e}");
    assert_failed_state(&agent, Some("DOCUMENT_ERROR"));
}

#[test]
fn document_agent_exposes_notion_tools() {
    // TS #45: tools は file_create, notion_page_create, notion_page_update の順
    let agent = DocumentAgent::new();
    assert_eq!(
        agent.base().get_available_tools(),
        ["file_create", "notion_page_create", "notion_page_update"]
    );
    assert_eq!(
        agent.get_config().tools,
        ["file_create", "notion_page_create", "notion_page_update"]
    );
}
