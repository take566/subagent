//! MCP サーバー実装（rmcp / stdio）
//!
//! Subagent のツールと、タスクを丸ごと実行する `execute` ツールを MCP で公開する。
//!
//! 環境変数（TS #40）:
//! - `AGENT_ID`: 指定があれば agent の id を上書きする（前後の空白は除く、空なら無視）
//! - `MAX_CONCURRENT`: tools/call の同時実行数の上限（正の整数。未設定・不正値は無制限）

use crate::core::subagent::Subagent;
use crate::types::{JsonMap, SubagentConfig, Task};
use crate::utils::logger::Logger;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool as McpTool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler, ServiceExt};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Semaphore;

/// MCP サーバーの環境変数設定
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpEnv {
    /// `AGENT_ID`（trim 済み、空なら None）
    pub agent_id: Option<String>,
    /// `MAX_CONCURRENT`（正の整数のときだけ Some）
    pub max_concurrent: Option<usize>,
}

impl McpEnv {
    /// プロセスの環境変数から読む
    pub fn from_env() -> Self {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// 任意の参照関数から読む（テスト用）
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let agent_id = get("AGENT_ID")
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        // TS: `Number.parseInt(v, 10)` が正の整数のときだけ有効
        let max_concurrent = get("MAX_CONCURRENT")
            .and_then(|v| parse_int_prefix(&v))
            .filter(|n| *n > 0)
            .and_then(|n| usize::try_from(n).ok());
        McpEnv {
            agent_id,
            max_concurrent,
        }
    }

    /// `AGENT_ID` を config に反映する（TS の `subagent.setId`）。
    ///
    /// Rust 版はエージェントを作る前の config に入れるので、state / logger の id にも反映される。
    pub fn apply_to(&self, config: &mut SubagentConfig) {
        if let Some(id) = &self.agent_id {
            config.id = id.clone();
        }
    }
}

/// JS の `Number.parseInt(s, 10)` 相当: 先頭の空白を飛ばし、符号と続く数字だけを読む
fn parse_int_prefix(s: &str) -> Option<i128> {
    let s = s.trim_start();
    let (neg, rest) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    // 桁が多すぎる値は飽和させる（上限は Semaphore 側で丸める）
    let n = digits.parse::<i128>().unwrap_or(i128::MAX);
    Some(if neg { -n } else { n })
}

/// execute ツールの引数検証（TS #54 の `validateExecuteArgs`）
fn validate_execute_args(args: &JsonMap) -> Result<(), &'static str> {
    if !matches!(args.get("action"), Some(Value::String(s)) if !s.is_empty()) {
        return Err("Invalid arguments: action must be a non-empty string");
    }
    if args
        .get("task_id")
        .is_some_and(|v| !matches!(v, Value::String(s) if !s.is_empty()))
    {
        return Err("Invalid arguments: task_id must be a non-empty string");
    }
    if args.get("parameters").is_some_and(|v| !v.is_object()) {
        return Err("Invalid arguments: parameters must be an object");
    }
    Ok(())
}

pub struct McpServer {
    subagent: Arc<dyn Subagent>,
    logger: Logger,
    limit: Option<Arc<Semaphore>>,
}

impl McpServer {
    /// 同時実行数の上限なし
    pub fn new(subagent: Arc<dyn Subagent>) -> Self {
        Self::with_max_concurrent(subagent, None)
    }

    /// tools/call の同時実行数を `max_concurrent` までに制限する（None は無制限）
    pub fn with_max_concurrent(subagent: Arc<dyn Subagent>, max_concurrent: Option<usize>) -> Self {
        let logger = Logger::new(format!("mcp-{}", subagent.get_config().id));
        let limit = max_concurrent
            .filter(|n| *n > 0)
            .map(|n| Arc::new(Semaphore::new(n.min(Semaphore::MAX_PERMITS))));
        McpServer {
            subagent,
            logger,
            limit,
        }
    }

    /// 公開するツール一覧（エージェントのツール + `execute`）
    pub fn tool_list(&self) -> Vec<McpTool> {
        let base = self.subagent.base();
        let mut tools: Vec<McpTool> = base
            .get_available_tools()
            .into_iter()
            .map(|name| {
                let tool = base.get_tool(&name);
                let description = tool
                    .as_ref()
                    .map(|t| t.description().to_string())
                    .unwrap_or_default();
                let properties = tool.map(|t| t.parameters().clone()).unwrap_or_default();
                McpTool::new(name, description, object_schema(properties))
            })
            .collect();
        tools.push(McpTool::new(
            "execute",
            "タスクを実行",
            object_schema(
                json!({
                    "task_id": { "type": "string", "description": "タスクID" },
                    "action": { "type": "string", "description": "アクション名" },
                    "parameters": { "type": "object", "description": "パラメータ" },
                })
                .as_object()
                .cloned()
                .unwrap_or_default(),
            ),
        ));
        tools
    }

    /// ツールを呼び出す。失敗は `isError: true` のテキストで返す（TS 版と同じ）。
    pub async fn call(&self, name: &str, args: Option<JsonMap>) -> CallToolResult {
        let Some(args) = args else {
            return CallToolResult::error(vec![ContentBlock::text("Error: Missing arguments")]);
        };
        if name == "execute"
            && let Err(msg) = validate_execute_args(&args)
        {
            return CallToolResult::error(vec![ContentBlock::text(format!("Error: {msg}"))]);
        }
        // 上限に達していれば空くまで待つ（引数エラーは枠を使わない。TS と同じ順序）
        let _permit = match &self.limit {
            Some(s) => s.clone().acquire_owned().await.ok(),
            None => None,
        };
        let outcome = if name == "execute" {
            // 引数は validate_execute_args で検証済み
            let task = Task {
                id: match args.get("task_id") {
                    Some(Value::String(s)) => s.clone(),
                    _ => format!("task-{}", chrono::Utc::now().timestamp_millis()),
                },
                action: args
                    .get("action")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .into(),
                parameters: args
                    .get("parameters")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default(),
                context: None,
                metadata: None,
            };
            self.subagent
                .execute(task)
                .await
                .and_then(|r| Ok(serde_json::to_value(r)?))
        } else {
            self.subagent.base().invoke_tool(name, args).await
        };
        match outcome {
            Ok(v) => CallToolResult::success(vec![ContentBlock::text(
                serde_json::to_string_pretty(&v).unwrap_or_default(),
            )]),
            Err(e) => {
                self.logger.error(
                    &format!("Tool execution failed: {name}"),
                    Some(&json!({ "error": e.to_string() })),
                );
                CallToolResult::error(vec![ContentBlock::text(format!("Error: {e}"))])
            }
        }
    }

    /// stdio でサーバーを起動し、クライアントが切断するまで待つ
    pub async fn start(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let agent = self.subagent.get_config().name.clone();
        let logger = self.logger.clone();
        let service = self.serve(rmcp::transport::stdio()).await?;
        logger.info("MCP Server started", Some(&json!({ "agent": agent })));
        service.waiting().await?;
        Ok(())
    }
}

fn object_schema(properties: JsonMap) -> JsonMap {
    let mut m = JsonMap::new();
    m.insert("type".into(), json!("object"));
    m.insert("properties".into(), Value::Object(properties));
    m
}

impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerConfig {
        let c = self.subagent.get_config();
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(c.name.clone(), c.version.clone()))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(self.tool_list()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        Ok(self.call(&request.name, request.arguments).await.into())
    }
}

/// エージェントを MCP サーバーとして起動する（バイナリのエントリーポイント用）。
///
/// `AGENT_ID` を `config` に反映してから `build` でエージェントを作り、
/// `MAX_CONCURRENT` で tools/call の同時実行数を制限する。
pub async fn run_stdio<A, F>(config: SubagentConfig, build: F, label: &str)
where
    A: Subagent + 'static,
    F: FnOnce(SubagentConfig) -> A,
{
    let env = McpEnv::from_env();
    let mut config = config;
    env.apply_to(&mut config);
    let agent: Arc<dyn Subagent> = Arc::new(build(config));
    if let Err(e) = McpServer::with_max_concurrent(agent, env.max_concurrent)
        .start()
        .await
    {
        eprintln!("Failed to start {label} MCP Server: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    //! TS 版 `src/mcp/server.test.ts`（#40 / #54）の移植
    use super::*;
    use crate::core::subagent::AgentBase;
    use crate::error::Result;
    use crate::types::{
        BehaviorConfig, InterfaceConfig, RoleConfig, TaskMetrics, TaskResult, TaskStatus,
    };
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::Duration;

    fn config() -> SubagentConfig {
        SubagentConfig {
            id: "orig".into(),
            name: "n".into(),
            version: "1".into(),
            role: RoleConfig {
                description: String::new(),
                capabilities: vec![],
                constraints: None,
            },
            interface: InterfaceConfig {
                input: vec![],
                output: vec![],
            },
            tools: vec![],
            behavior: BehaviorConfig::default(),
        }
    }

    #[derive(Default)]
    struct Counters {
        running: usize,
        peak: usize,
        calls: Vec<Task>,
    }

    struct FakeAgent {
        base: AgentBase,
        state: Mutex<Counters>,
    }

    impl FakeAgent {
        fn new(config: SubagentConfig) -> Self {
            FakeAgent {
                base: AgentBase::new(config, vec![]),
                state: Mutex::default(),
            }
        }
    }

    #[async_trait]
    impl Subagent for FakeAgent {
        fn base(&self) -> &AgentBase {
            &self.base
        }

        async fn execute(&self, task: Task) -> Result<TaskResult> {
            {
                let mut s = self.state.lock().unwrap();
                s.calls.push(task.clone());
                s.running += 1;
                s.peak = s.peak.max(s.running);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
            self.state.lock().unwrap().running -= 1;
            Ok(TaskResult {
                task_id: task.id,
                status: TaskStatus::Completed,
                result: Some(json!({ "ok": true })),
                error: None,
                confidence: None,
                metrics: Some(TaskMetrics::default()),
            })
        }
    }

    fn env(vars: &[(&str, &str)]) -> McpEnv {
        let m: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        McpEnv::from_lookup(|k| m.get(k).cloned())
    }

    /// TS の `new MCPServer(agent, env)` 相当: env を反映して agent と server を作る
    fn server_with(vars: &[(&str, &str)]) -> (McpServer, Arc<FakeAgent>) {
        let e = env(vars);
        let mut c = config();
        e.apply_to(&mut c);
        let agent = Arc::new(FakeAgent::new(c));
        let server = McpServer::with_max_concurrent(agent.clone(), e.max_concurrent);
        (server, agent)
    }

    fn as_json(r: &CallToolResult) -> Value {
        serde_json::to_value(r).unwrap()
    }

    async fn call_execute(server: &McpServer, args: Value) -> Value {
        as_json(&server.call("execute", args.as_object().cloned()).await)
    }

    #[test]
    fn agent_id_overrides_agent_id() {
        let (_, agent) = server_with(&[("AGENT_ID", "research-001")]);
        assert_eq!(agent.get_config().id, "research-001");
        assert_eq!(agent.get_state().id, "research-001");
        let (_, agent) = server_with(&[("AGENT_ID", "  spaced  ")]);
        assert_eq!(agent.get_config().id, "spaced");
    }

    #[test]
    fn missing_or_blank_agent_id_keeps_id() {
        let (_, agent) = server_with(&[]);
        assert_eq!(agent.get_config().id, "orig");
        let (_, agent) = server_with(&[("AGENT_ID", "   ")]);
        assert_eq!(agent.get_config().id, "orig");
    }

    #[test]
    fn max_concurrent_parsing_follows_parse_int() {
        let mc = |v: &str| env(&[("MAX_CONCURRENT", v)]).max_concurrent;
        assert_eq!(mc("2"), Some(2));
        assert_eq!(mc(" 3"), Some(3));
        assert_eq!(mc("4abc"), Some(4));
        assert_eq!(mc("+5"), Some(5));
        assert_eq!(mc("abc"), None);
        assert_eq!(mc("0"), None);
        assert_eq!(mc("-1"), None);
        assert_eq!(mc(""), None);
        assert_eq!(env(&[]).max_concurrent, None);
    }

    #[tokio::test]
    async fn max_concurrent_limits_parallel_calls() {
        let (server, agent) = server_with(&[("MAX_CONCURRENT", "2")]);
        let calls = (0..6).map(|_| call_execute(&server, json!({ "action": "x" })));
        futures::future::join_all(calls).await;
        assert_eq!(agent.state.lock().unwrap().peak, 2);
        assert_eq!(agent.state.lock().unwrap().calls.len(), 6);
    }

    #[tokio::test]
    async fn invalid_max_concurrent_is_unlimited() {
        let (server, agent) = server_with(&[("MAX_CONCURRENT", "abc")]);
        let calls = (0..4).map(|_| call_execute(&server, json!({ "action": "x" })));
        futures::future::join_all(calls).await;
        assert_eq!(agent.state.lock().unwrap().peak, 4);
    }

    #[tokio::test]
    async fn invalid_execute_args_return_error_without_executing() {
        let cases = [
            ("action 欠落", json!({}), "action"),
            ("action が数値", json!({ "action": 1 }), "action"),
            ("action が空文字", json!({ "action": "" }), "action"),
            (
                "parameters が配列",
                json!({ "action": "x", "parameters": [] }),
                "parameters",
            ),
            (
                "parameters が文字列",
                json!({ "action": "x", "parameters": "a" }),
                "parameters",
            ),
            (
                "parameters が null",
                json!({ "action": "x", "parameters": null }),
                "parameters",
            ),
            (
                "task_id が数値",
                json!({ "action": "x", "task_id": 1 }),
                "task_id",
            ),
            (
                "task_id が空文字",
                json!({ "action": "x", "task_id": "" }),
                "task_id",
            ),
        ];
        for (label, args, arg_name) in cases {
            let (server, agent) = server_with(&[]);
            let r = call_execute(&server, args).await;
            assert_eq!(r["isError"], true, "{label}: {r}");
            let text = r["content"][0]["text"].as_str().unwrap();
            assert!(
                text.starts_with("Error: Invalid arguments: "),
                "{label}: {text}"
            );
            assert!(text.contains(arg_name), "{label}: {text}");
            assert!(agent.state.lock().unwrap().calls.is_empty(), "{label}");
        }
    }

    #[tokio::test]
    async fn valid_args_are_passed_to_execute() {
        let (server, agent) = server_with(&[]);
        let r = call_execute(
            &server,
            json!({ "action": "x", "parameters": { "a": 1 }, "task_id": "t1" }),
        )
        .await;
        assert!(r.get("isError").is_none_or(|v| v == false), "{r}");
        let calls = &agent.state.lock().unwrap().calls;
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "t1");
        assert_eq!(calls[0].action, "x");
        assert_eq!(
            Value::Object(calls[0].parameters.clone()),
            json!({ "a": 1 })
        );
    }

    #[tokio::test]
    async fn action_only_is_enough() {
        let (server, agent) = server_with(&[]);
        let r = call_execute(&server, json!({ "action": "x" })).await;
        assert!(r.get("isError").is_none_or(|v| v == false), "{r}");
        let calls = &agent.state.lock().unwrap().calls;
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].action, "x");
        assert!(calls[0].parameters.is_empty());
        assert!(calls[0].id.starts_with("task-"));
    }
}
