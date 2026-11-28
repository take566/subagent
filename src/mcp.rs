//! MCP サーバー実装（rmcp / stdio）
//!
//! 公開するツールは `SUBAGENT_MCP_MODE` で決まる:
//! - `hermes`（既定。不明な値もこれ）: エージェントの typed tool（[`Subagent::typed_tool`]）だけ。
//!   それ以外のツール（direct tool と `execute`）の呼び出しは拒否する
//! - `full`: エージェントのツール（direct tool）と、タスクを丸ごと実行する `execute`、typed tool
//!
//! 環境変数（TS #40 ほか）:
//! - `AGENT_ID`: 指定があれば agent の id を上書きする（前後の空白は除く、空なら無視）
//! - `MAX_CONCURRENT`: tools/call の同時実行数の上限（正の整数。未設定・不正値は無制限）
//! - `SUBAGENT_MCP_MODE`: 上の mode（前後の空白と大文字小文字は問わない）
//! - `SUBAGENT_DEADLINE_SECS`: 1 回の tools/call の deadline（1〜86400 の整数の秒。既定 540。それ以外は起動しない）。
//!   request を受けた時から数える（同時実行数の待ちも含む）。agent の `behavior.timeout`
//!   があれば短い方を使う
//!
//! request の `_meta.progressToken` があれば `notifications/progress` を送る（開始・runner の
//! 往復ごと・終了）。終了の通知は応答の前に送り、応答の後には送らない。

use crate::core::call::{CallCtx, ProgressNotifier, ProgressSink};
use crate::core::subagent::Subagent;
use crate::llm::runner::validate_against_schema;
use crate::llm::{self, AgentKind, AgentLlmSettings, LlmClient};
use crate::types::{JsonMap, SubagentConfig, Task};
use crate::utils::logger::Logger;
use async_trait::async_trait;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    ListToolsResult, PaginatedRequestParams, ProgressNotificationParam, ProgressToken,
    ServerCapabilities, ServerConfig, Tool as McpTool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, Peer, RoleServer, ServerHandler, ServiceExt};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// mode を選ぶ env
pub const MCP_MODE_ENV: &str = "SUBAGENT_MCP_MODE";
/// deadline（秒）の env
pub const DEADLINE_ENV: &str = "SUBAGENT_DEADLINE_SECS";
/// deadline の既定（秒）
pub const DEFAULT_DEADLINE_SECS: u64 = 540;
/// deadline の上限（秒、1 日）。これより大きい値は設定エラー
pub const MAX_DEADLINE_SECS: u64 = 86_400;
/// 開始・終了・取り消しの進捗通知を送るのを待つ上限（stdout が詰まっても呼び出しを止めない）
const PROGRESS_NOTIFY_TIMEOUT: Duration = Duration::from_secs(2);

/// MCP の env の設定エラー（bin は起動しない）
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct McpEnvError(pub String);

/// 公開するツールの範囲
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum McpMode {
    /// typed tool だけ（既定）
    #[default]
    Hermes,
    /// direct tool と `execute` も
    Full,
}

impl McpMode {
    /// `full`（前後の空白・大文字小文字は問わない）以外は全部 Hermes（fail closed）
    pub fn parse(value: Option<&str>) -> Self {
        match value.map(|v| v.trim().to_ascii_lowercase()) {
            Some(v) if v == "full" => McpMode::Full,
            _ => McpMode::Hermes,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            McpMode::Hermes => "hermes",
            McpMode::Full => "full",
        }
    }
}

/// MCP サーバーの環境変数設定
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpEnv {
    /// `AGENT_ID`（trim 済み、空なら None）
    pub agent_id: Option<String>,
    /// `MAX_CONCURRENT`（正の整数のときだけ Some）
    pub max_concurrent: Option<usize>,
    /// `SUBAGENT_MCP_MODE`
    pub mode: McpMode,
    /// `SUBAGENT_DEADLINE_SECS`（正の整数のときだけ。それ以外は既定の 540 秒）
    pub deadline: Duration,
}

impl Default for McpEnv {
    fn default() -> Self {
        Self {
            agent_id: None,
            max_concurrent: None,
            mode: McpMode::Hermes,
            deadline: Duration::from_secs(DEFAULT_DEADLINE_SECS),
        }
    }
}

impl McpEnv {
    /// プロセスの環境変数から読む
    pub fn from_env() -> Result<Self, McpEnvError> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// 任意の参照関数から読む（テスト用）。
    ///
    /// `SUBAGENT_DEADLINE_SECS` が 1〜86400 の整数でなければエラー（未設定・空は既定の 540）
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, McpEnvError> {
        let agent_id = get("AGENT_ID")
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        // TS: `Number.parseInt(v, 10)` が正の整数のときだけ有効
        let max_concurrent = get("MAX_CONCURRENT")
            .and_then(|v| parse_int_prefix(&v))
            .filter(|n| *n > 0)
            .and_then(|n| usize::try_from(n).ok());
        let deadline = match get(DEADLINE_ENV)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
        {
            None => DEFAULT_DEADLINE_SECS,
            Some(raw) => raw
                .parse::<u64>()
                .ok()
                .filter(|n| (1..=MAX_DEADLINE_SECS).contains(n))
                .ok_or_else(|| {
                    McpEnvError(format!(
                        "{DEADLINE_ENV} must be an integer number of seconds between 1 and {MAX_DEADLINE_SECS}"
                    ))
                })?,
        };
        Ok(McpEnv {
            agent_id,
            max_concurrent,
            mode: McpMode::parse(get(MCP_MODE_ENV).as_deref()),
            deadline: Duration::from_secs(deadline),
        })
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

fn error_result(text: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(format!("Error: {}", text.into()))])
}

pub struct McpServer {
    subagent: Arc<dyn Subagent>,
    logger: Logger,
    limit: Option<Arc<Semaphore>>,
    mode: McpMode,
    deadline: Duration,
}

impl McpServer {
    /// 同時実行数の上限なし、hermes mode、既定の deadline
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
            mode: McpMode::Hermes,
            deadline: Duration::from_secs(DEFAULT_DEADLINE_SECS),
        }
    }

    /// env（`MAX_CONCURRENT` / `SUBAGENT_MCP_MODE` / `SUBAGENT_DEADLINE_SECS`）どおりに作る
    pub fn from_env(subagent: Arc<dyn Subagent>, env: &McpEnv) -> Self {
        Self::with_max_concurrent(subagent, env.max_concurrent)
            .with_mode(env.mode)
            .with_deadline(env.deadline)
    }

    pub fn with_mode(mut self, mode: McpMode) -> Self {
        self.mode = mode;
        self
    }

    /// 1 回の tools/call の deadline（`behavior.timeout` が短ければそちら）
    pub fn with_deadline(mut self, deadline: Duration) -> Self {
        self.deadline = deadline;
        self
    }

    pub fn mode(&self) -> McpMode {
        self.mode
    }

    /// request を受けた時刻からの deadline。`behavior.timeout` は縮めるだけ
    fn deadline_from(&self, received: Instant) -> Instant {
        let mut d = self.deadline;
        if let Some(ms) = self.subagent.get_config().behavior.timeout {
            d = d.min(Duration::from_millis(ms));
        }
        // 加算があふれる（with_deadline に巨大な値を渡した）時は遠い未来に飽和させる
        received
            .checked_add(d)
            .or_else(|| received.checked_add(Duration::from_secs(MAX_DEADLINE_SECS * 365 * 30)))
            .unwrap_or(received)
    }

    fn typed_tool(&self) -> Option<McpTool> {
        self.subagent.typed_tool()
    }

    /// 公開するツール一覧（mode による）
    pub fn tool_list(&self) -> Vec<McpTool> {
        let typed = self.typed_tool();
        if self.mode == McpMode::Hermes {
            return typed.into_iter().collect();
        }
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
        // validate_execute_args と同じ制約を schema にも書く（クライアントが送る前に分かるように）。
        // ほかのツールは TS 版どおり property ごとの `required: true` の形のまま
        let mut execute_schema = object_schema(
            json!({
                "task_id": { "type": "string", "minLength": 1, "description": "タスクID" },
                "action": { "type": "string", "minLength": 1, "description": "アクション名" },
                "parameters": { "type": "object", "description": "パラメータ" },
            })
            .as_object()
            .cloned()
            .unwrap_or_default(),
        );
        execute_schema.insert("required".into(), json!(["action"]));
        tools.push(McpTool::new("execute", "タスクを実行", execute_schema));
        tools.extend(typed);
        tools
    }

    /// ツールを呼び出す（進捗なし・取り消しなし、受けた時刻は今）。失敗は `isError: true` のテキスト
    pub async fn call(&self, name: &str, args: Option<JsonMap>) -> CallToolResult {
        self.handle_call(
            name,
            args,
            Instant::now(),
            CancellationToken::new(),
            ProgressSink::none(),
        )
        .await
    }

    /// ツールを呼び出す。`received` から deadline を数え、`cancel` で打ち切る。
    ///
    /// `progress` に開始と終了を送る（終了は応答を返す前。その後の進捗は捨てる）。
    pub async fn handle_call(
        &self,
        name: &str,
        args: Option<JsonMap>,
        received: Instant,
        cancel: CancellationToken,
        progress: ProgressSink,
    ) -> CallToolResult {
        let deadline = self.deadline_from(received);
        // 通知が詰まっても呼び出しを止めない（送れなければその通知は諦める）
        let _ = tokio::time::timeout(
            PROGRESS_NOTIFY_TIMEOUT,
            progress.report(format!("started {name}")),
        )
        .await;
        let ctx = CallCtx {
            deadline,
            progress: progress.clone(),
            cancel: cancel.clone(),
        };
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                self.logger.warn("Tool call cancelled", Some(&json!({ "tool": name })));
                // 取り消した client には、終了も含めてもう何も送らない
                let _ = tokio::time::timeout(PROGRESS_NOTIFY_TIMEOUT, progress.close()).await;
                return error_result("Request cancelled");
            }
            r = tokio::time::timeout_at(deadline, self.dispatch(name, args, ctx)) => match r {
                Ok(r) => r,
                Err(_) => {
                    let secs = deadline.saturating_duration_since(received).as_secs_f64();
                    self.logger.error(
                        "Tool call exceeded the deadline",
                        Some(&json!({ "tool": name, "deadline_secs": secs })),
                    );
                    error_result(format!(
                        "Deadline exceeded after {secs}s ({DEADLINE_ENV})"
                    ))
                }
            },
        };
        let failed = result.is_error == Some(true);
        // 送れなくても（stdout が詰まっても）応答は返す。finish は送る前に sink を閉じるので、
        // 諦めた後の進捗も届かない
        let _ = tokio::time::timeout(
            PROGRESS_NOTIFY_TIMEOUT,
            progress.finish(if failed { "failed" } else { "completed" }),
        )
        .await;
        result
    }

    async fn dispatch(&self, name: &str, args: Option<JsonMap>, ctx: CallCtx) -> CallToolResult {
        let typed = self.typed_tool();
        let is_typed = typed.as_ref().is_some_and(|t| t.name == name);
        if self.mode == McpMode::Hermes && !is_typed {
            let listed = typed
                .as_ref()
                .map(|t| format!("`{}`", t.name))
                .unwrap_or_else(|| "no tools".into());
            return error_result(format!(
                "Tool {name} is not available ({MCP_MODE_ENV}=hermes lists only {listed})"
            ));
        }
        let Some(args) = args else {
            return error_result("Missing arguments");
        };
        if let Some(t) = typed.as_ref().filter(|_| is_typed)
            && let Err(reason) = validate_against_schema(
                &Value::Object((*t.input_schema).clone()),
                &Value::Object(args.clone()),
            )
        {
            return error_result(format!("Invalid arguments: {reason}"));
        }
        if name == "execute"
            && let Err(msg) = validate_execute_args(&args)
        {
            return error_result(msg);
        }
        // 上限に達していれば空くまで待つ（引数エラーは枠を使わない。TS と同じ順序）
        let _permit = match &self.limit {
            Some(s) => s.clone().acquire_owned().await.ok(),
            None => None,
        };
        let outcome = if is_typed {
            self.subagent.call_typed(Value::Object(args), ctx).await
        } else if name == "execute" {
            // 引数は validate_execute_args で検証済み
            let task = Task {
                id: match args.get("task_id") {
                    Some(Value::String(s)) => s.clone(),
                    _ => format!("task-{}", uuid::Uuid::new_v4()),
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
                error_result(e.to_string())
            }
        }
    }

    /// stdio でサーバーを起動し、クライアントが切断するまで待つ
    pub async fn start(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let agent = self.subagent.get_config().name.clone();
        let logger = self.logger.clone();
        let mode = self.mode.as_str();
        let deadline_secs = self.deadline.as_secs();
        let service = self.serve(rmcp::transport::stdio()).await?;
        logger.info(
            "MCP Server started",
            Some(&json!({ "agent": agent, "mode": mode, "deadline_secs": deadline_secs })),
        );
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

/// `notifications/progress` を client に送る
struct PeerProgress {
    peer: Peer<RoleServer>,
    token: ProgressToken,
}

#[async_trait]
impl ProgressNotifier for PeerProgress {
    async fn notify(&self, progress: f64, total: Option<f64>, message: Option<String>) {
        let mut param = ProgressNotificationParam::new(self.token.clone(), progress);
        param.total = total;
        param.message = message;
        // 送れなくても（client が切断した等）呼び出し自体は続ける
        let _ = self.peer.notify_progress(param).await;
    }
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
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let received = Instant::now();
        let token = context
            .meta
            .get_progress_token()
            .or_else(|| request.meta.as_ref().and_then(|m| m.get_progress_token()));
        let progress = match token {
            Some(token) => ProgressSink::new(Arc::new(PeerProgress {
                peer: context.peer.clone(),
                token,
            })),
            None => ProgressSink::none(),
        };
        Ok(self
            .handle_call(
                &request.name,
                request.arguments,
                received,
                context.ct.clone(),
                progress,
            )
            .await
            .into())
    }
}

/// 設定エラーで起動しない（値は載せない。exit code 2）
fn exit_config_error(label: &str, error: &dyn std::fmt::Display) -> ! {
    eprintln!("Failed to start {label} MCP Server: invalid configuration: {error}");
    std::process::exit(2);
}

/// エージェントを MCP サーバーとして起動する（バイナリのエントリーポイント用）。
///
/// `AGENT_ID` を `config` に反映し、LLM の client（[`llm::client_from_env`]、process で 1 つ）と
/// 設定（[`AgentLlmSettings::from_env`]）を読んでから `build` でエージェントを作る。
/// 設定が不正（不明な `SUBAGENT_LLM_BACKEND`、`replay` feature 無しの `replay`、不正な
/// effort / token 予算など）なら起動せずに exit code 2 で終わる。
pub async fn run_stdio<A, F>(config: SubagentConfig, kind: AgentKind, build: F, label: &str)
where
    A: Subagent + 'static,
    F: FnOnce(SubagentConfig, Arc<dyn LlmClient>, AgentLlmSettings) -> A,
{
    let env = McpEnv::from_env().unwrap_or_else(|e| exit_config_error(label, &e));
    let mut config = config;
    env.apply_to(&mut config);
    let client = llm::client_from_env().unwrap_or_else(|e| exit_config_error(label, &e));
    let settings =
        AgentLlmSettings::from_env(kind).unwrap_or_else(|e| exit_config_error(label, &e));
    let agent: Arc<dyn Subagent> = Arc::new(build(config, client, settings));
    if let Err(e) = McpServer::from_env(agent, &env).start().await {
        eprintln!("Failed to start {label} MCP Server: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    //! TS 版 `src/mcp/server.test.ts`（#40 / #54）の移植と、mode / deadline / progress / cancel
    use super::*;
    use crate::core::subagent::AgentBase;
    use crate::error::Result;
    use crate::types::{
        BehaviorConfig, InterfaceConfig, RoleConfig, TaskMetrics, TaskResult, TaskStatus,
    };
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::Mutex;

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
        typed_calls: Vec<Value>,
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

        fn typed_tool(&self) -> Option<McpTool> {
            let Value::Object(schema) = json!({
                "type": "object",
                "properties": {
                    "q": { "type": "string" },
                    "sleep_secs": { "type": "integer" },
                    "late_progress": { "type": "boolean" }
                },
                "required": ["q"],
                "additionalProperties": false
            }) else {
                unreachable!()
            };
            Some(McpTool::new("fake", "fake typed tool", schema))
        }

        /// `sleep_secs` 秒眠ってから返す。`late_progress` なら応答の後に進捗を送ろうとする
        async fn call_typed(&self, args: Value, ctx: CallCtx) -> Result<Value> {
            self.state.lock().unwrap().typed_calls.push(args.clone());
            ctx.progress.report("working").await;
            if args["late_progress"] == true {
                let p = ctx.progress.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    p.report("late").await;
                });
            }
            if let Some(s) = args["sleep_secs"].as_u64() {
                tokio::time::sleep(Duration::from_secs(s)).await;
            }
            Ok(json!({ "q": args["q"] }))
        }
    }

    fn env(vars: &[(&str, &str)]) -> McpEnv {
        let m: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        McpEnv::from_lookup(|k| m.get(k).cloned()).unwrap()
    }

    /// TS の `new MCPServer(agent, env)` 相当: env を反映して agent と server を作る（mode は full）
    fn server_with(vars: &[(&str, &str)]) -> (McpServer, Arc<FakeAgent>) {
        let mut vars = vars.to_vec();
        vars.push((MCP_MODE_ENV, "full"));
        server_from(&vars, config())
    }

    fn server_from(vars: &[(&str, &str)], mut c: SubagentConfig) -> (McpServer, Arc<FakeAgent>) {
        let e = env(vars);
        e.apply_to(&mut c);
        let agent = Arc::new(FakeAgent::new(c));
        let server = McpServer::from_env(agent.clone(), &e);
        (server, agent)
    }

    fn as_json(r: &CallToolResult) -> Value {
        serde_json::to_value(r).unwrap()
    }

    fn text_of(r: &Value) -> &str {
        r["content"][0]["text"].as_str().unwrap()
    }

    async fn call_execute(server: &McpServer, args: Value) -> Value {
        as_json(&server.call("execute", args.as_object().cloned()).await)
    }

    async fn call_fake(server: &McpServer, args: Value) -> Value {
        as_json(&server.call("fake", args.as_object().cloned()).await)
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

    #[test]
    fn mode_parsing_fails_closed_to_hermes() {
        let mode = |v: &str| env(&[(MCP_MODE_ENV, v)]).mode;
        assert_eq!(env(&[]).mode, McpMode::Hermes);
        assert_eq!(mode("full"), McpMode::Full);
        assert_eq!(mode(" Full\n"), McpMode::Full);
        for v in ["", "hermes", "ful", "full2", "all", "FULL MODE"] {
            assert_eq!(mode(v), McpMode::Hermes, "{v:?}");
        }
    }

    #[test]
    fn deadline_parsing() {
        let parse = |v: &str| {
            let m: HashMap<String, String> = [(DEADLINE_ENV.to_string(), v.to_string())].into();
            McpEnv::from_lookup(|k| m.get(k).cloned()).map(|e| e.deadline)
        };
        assert_eq!(env(&[]).deadline, Duration::from_secs(540));
        assert_eq!(parse("").unwrap(), Duration::from_secs(540));
        assert_eq!(parse("30").unwrap(), Duration::from_secs(30));
        assert_eq!(parse(" 60 ").unwrap(), Duration::from_secs(60));
        assert_eq!(parse("86400").unwrap(), Duration::from_secs(86_400));
        for v in [
            "0",
            "-5",
            "abc",
            "300s",
            "1.5",
            "86401",
            "18446744073709551615",
            "99999999999999999999999",
        ] {
            let e = parse(v).unwrap_err();
            assert!(e.to_string().contains(DEADLINE_ENV), "{v:?}: {e}");
        }
    }

    #[test]
    fn hermes_lists_only_the_typed_tool_and_full_lists_everything() {
        let (hermes, _) = server_from(&[], config());
        let names = |s: &McpServer| -> Vec<String> {
            s.tool_list().iter().map(|t| t.name.to_string()).collect()
        };
        assert_eq!(names(&hermes), ["fake"]);
        let (full, _) = server_with(&[]);
        assert_eq!(names(&full), ["execute", "fake"]);
    }

    #[tokio::test]
    async fn hermes_rejects_execute_and_direct_tools() {
        let (server, agent) = server_from(&[], config());
        for name in ["execute", "web_search", "nope"] {
            let r = as_json(
                &server
                    .call(name, json!({ "action": "x" }).as_object().cloned())
                    .await,
            );
            assert_eq!(r["isError"], true, "{name}: {r}");
            assert!(text_of(&r).contains("SUBAGENT_MCP_MODE=hermes"), "{r}");
        }
        assert!(agent.state.lock().unwrap().calls.is_empty());
        let r = call_fake(&server, json!({ "q": "x" })).await;
        assert!(r.get("isError").is_none_or(|v| v == false), "{r}");
    }

    #[tokio::test]
    async fn typed_args_are_validated_before_the_agent_is_called() {
        let (server, agent) = server_from(&[], config());
        for (args, needle) in [
            (json!({}), "missing required field `q`"),
            (json!({ "q": 1 }), "field `q` must be of type"),
            (json!({ "q": "x", "extra": 1 }), "unexpected field `extra`"),
        ] {
            let r = call_fake(&server, args.clone()).await;
            assert_eq!(r["isError"], true, "{args}: {r}");
            let text = text_of(&r);
            assert!(text.starts_with("Error: Invalid arguments: "), "{text}");
            assert!(text.contains(needle), "{text}");
        }
        assert!(agent.state.lock().unwrap().typed_calls.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn deadline_stops_a_slow_typed_call() {
        let (server, _) = server_from(&[(DEADLINE_ENV, "5")], config());
        let started = Instant::now();
        let r = call_fake(&server, json!({ "q": "x", "sleep_secs": 1000 })).await;
        assert_eq!(r["isError"], true, "{r}");
        assert!(text_of(&r).contains("Deadline exceeded"), "{r}");
        assert!(text_of(&r).contains(DEADLINE_ENV), "{r}");
        assert_eq!(started.elapsed(), Duration::from_secs(5));
        // 間に合う呼び出しはそのまま
        let r = call_fake(&server, json!({ "q": "x", "sleep_secs": 4 })).await;
        assert!(r.get("isError").is_none_or(|v| v == false), "{r}");
    }

    #[tokio::test(start_paused = true)]
    async fn behavior_timeout_only_shortens_the_deadline() {
        let mut c = config();
        c.behavior.timeout = Some(2_000);
        let (server, _) = server_from(&[(DEADLINE_ENV, "5")], c);
        let started = Instant::now();
        let r = call_fake(&server, json!({ "q": "x", "sleep_secs": 1000 })).await;
        assert_eq!(r["isError"], true, "{r}");
        assert_eq!(started.elapsed(), Duration::from_secs(2));

        let mut c = config();
        c.behavior.timeout = Some(60_000);
        let (server, _) = server_from(&[(DEADLINE_ENV, "5")], c);
        let started = Instant::now();
        let r = call_fake(&server, json!({ "q": "x", "sleep_secs": 1000 })).await;
        assert_eq!(r["isError"], true, "{r}");
        assert_eq!(started.elapsed(), Duration::from_secs(5));
    }

    /// deadline は request を受けた時から数える（同時実行数の待ちも含む）
    #[tokio::test(start_paused = true)]
    async fn deadline_covers_the_concurrency_wait() {
        let (server, _) = server_from(&[(DEADLINE_ENV, "5"), ("MAX_CONCURRENT", "1")], config());
        let started = Instant::now();
        let first = call_fake(&server, json!({ "q": "a", "sleep_secs": 4 }));
        let second = call_fake(&server, json!({ "q": "b", "sleep_secs": 4 }));
        let (a, b) = tokio::join!(first, second);
        assert!(a.get("isError").is_none_or(|v| v == false), "{a}");
        assert_eq!(b["isError"], true, "{b}");
        assert!(text_of(&b).contains("Deadline exceeded"), "{b}");
        assert_eq!(started.elapsed(), Duration::from_secs(5));
    }

    #[tokio::test(start_paused = true)]
    async fn cancel_stops_the_call() {
        let (server, _) = server_from(&[], config());
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(3)).await;
            c2.cancel();
        });
        let started = Instant::now();
        let r = as_json(
            &server
                .handle_call(
                    "fake",
                    json!({ "q": "x", "sleep_secs": 1000 }).as_object().cloned(),
                    Instant::now(),
                    cancel,
                    ProgressSink::none(),
                )
                .await,
        );
        assert_eq!(r["isError"], true, "{r}");
        assert!(text_of(&r).contains("cancelled"), "{r}");
        assert_eq!(started.elapsed(), Duration::from_secs(3));
    }

    type Sent = Vec<(f64, Option<f64>, Option<String>)>;

    #[derive(Default)]
    struct RecordedProgress(Mutex<Sent>);

    #[async_trait]
    impl ProgressNotifier for RecordedProgress {
        async fn notify(&self, progress: f64, total: Option<f64>, message: Option<String>) {
            self.0.lock().unwrap().push((progress, total, message));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn progress_increases_ends_before_the_response_and_stops_after_it() {
        let (server, _) = server_from(&[], config());
        let rec = Arc::new(RecordedProgress::default());
        let r = as_json(
            &server
                .handle_call(
                    "fake",
                    json!({ "q": "x", "late_progress": true })
                        .as_object()
                        .cloned(),
                    Instant::now(),
                    CancellationToken::new(),
                    ProgressSink::new(rec.clone()),
                )
                .await,
        );
        assert!(r.get("isError").is_none_or(|v| v == false), "{r}");
        let at_response = rec.0.lock().unwrap().clone();
        // 開始・agent の進捗・終了
        assert_eq!(at_response.len(), 3, "{at_response:?}");
        assert!(at_response.windows(2).all(|w| w[0].0 < w[1].0));
        let last = at_response.last().unwrap();
        assert_eq!(last.1, Some(last.0), "the last notification carries total");
        assert_eq!(last.2.as_deref(), Some("completed"));
        // agent が応答の後に送ろうとした進捗は届かない
        tokio::time::sleep(Duration::from_secs(5)).await;
        assert_eq!(*rec.0.lock().unwrap(), at_response);
    }

    /// 巨大な deadline でも Instant の加算で panic せず、飽和させて普通に返す
    #[tokio::test]
    async fn huge_deadline_saturates_instead_of_panicking() {
        let (server, _) = server_from(&[], config());
        let server = server.with_deadline(Duration::from_secs(u64::MAX));
        let r = call_fake(&server, json!({ "q": "x" })).await;
        assert!(r.get("isError").is_none_or(|v| v == false), "{r}");
    }

    /// client が取り消した後は、終了の通知も含めて何も送らない
    #[tokio::test(start_paused = true)]
    async fn no_progress_after_cancel() {
        let (server, _) = server_from(&[], config());
        let rec = Arc::new(RecordedProgress::default());
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            c2.cancel();
        });
        let r = as_json(
            &server
                .handle_call(
                    "fake",
                    json!({ "q": "x", "sleep_secs": 1000, "late_progress": true })
                        .as_object()
                        .cloned(),
                    Instant::now(),
                    cancel,
                    ProgressSink::new(rec.clone()),
                )
                .await,
        );
        assert_eq!(r["isError"], true, "{r}");
        tokio::time::sleep(Duration::from_secs(10)).await;
        let messages: Vec<String> = rec
            .0
            .lock()
            .unwrap()
            .iter()
            .map(|s| s.2.clone().unwrap_or_default())
            .collect();
        // cancel（0.5 秒後）より前に送ったものだけ。late（1 秒後）は cancel の後なので届かない
        assert_eq!(messages, ["started fake", "working"], "{messages:?}");
    }

    /// 通知を送れない（stdout が詰まった）時も、deadline を大きく過ぎて呼び出しを止めない
    #[tokio::test(start_paused = true)]
    async fn stuck_progress_does_not_hold_the_call_past_its_deadline() {
        struct Stuck;
        #[async_trait]
        impl ProgressNotifier for Stuck {
            async fn notify(&self, _: f64, _: Option<f64>, _: Option<String>) {
                tokio::time::sleep(Duration::from_secs(100_000)).await;
            }
        }
        let (server, _) = server_from(&[(DEADLINE_ENV, "5")], config());
        let started = Instant::now();
        let r = as_json(
            &server
                .handle_call(
                    "fake",
                    json!({ "q": "x" }).as_object().cloned(),
                    Instant::now(),
                    CancellationToken::new(),
                    ProgressSink::new(Arc::new(Stuck)),
                )
                .await,
        );
        assert!(r.get("content").is_some(), "{r}");
        // 開始 2 秒 + deadline 5 秒（agent の進捗は deadline の中）+ 終了 2 秒 まで
        assert!(
            started.elapsed() <= Duration::from_secs(9),
            "{:?}",
            started.elapsed()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn progress_ends_with_failed_on_errors_and_deadline() {
        let (server, _) = server_from(&[(DEADLINE_ENV, "1")], config());
        for args in [json!({}), json!({ "q": "x", "sleep_secs": 100 })] {
            let rec = Arc::new(RecordedProgress::default());
            let r = as_json(
                &server
                    .handle_call(
                        "fake",
                        args.as_object().cloned(),
                        Instant::now(),
                        CancellationToken::new(),
                        ProgressSink::new(rec.clone()),
                    )
                    .await,
            );
            assert_eq!(r["isError"], true);
            let got = rec.0.lock().unwrap().clone();
            assert!(got.len() >= 2, "{got:?}");
            assert_eq!(got.last().unwrap().2.as_deref(), Some("failed"));
        }
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
        // 既定の task id は uuid
        let id = calls[0].id.strip_prefix("task-").unwrap();
        assert!(uuid::Uuid::parse_str(id).is_ok(), "{id}");
    }

    #[test]
    fn execute_schema_requires_non_empty_action() {
        let (server, _) = server_with(&[]);
        let tools = server.tool_list();
        let execute = tools.iter().find(|t| t.name == "execute").unwrap();
        let schema = Value::Object((*execute.input_schema).clone());
        assert_eq!(schema["required"], json!(["action"]));
        assert_eq!(schema["properties"]["action"]["minLength"], json!(1));
        assert_eq!(schema["properties"]["task_id"]["minLength"], json!(1));
    }
}
