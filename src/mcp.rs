//! MCP サーバー実装（rmcp / stdio）
//!
//! Subagent のツールと、タスクを丸ごと実行する `execute` ツールを MCP で公開する。

use crate::core::subagent::Subagent;
use crate::types::{JsonMap, Task};
use crate::utils::logger::Logger;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool as McpTool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler, ServiceExt};
use serde_json::{Value, json};
use std::sync::Arc;

pub struct McpServer {
    subagent: Arc<dyn Subagent>,
    logger: Logger,
}

impl McpServer {
    pub fn new(subagent: Arc<dyn Subagent>) -> Self {
        let logger = Logger::new(format!("mcp-{}", subagent.get_config().id));
        McpServer { subagent, logger }
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
        let outcome = if name == "execute" {
            let task = Task {
                // TS: `args.task_id || ...` — truthy な値ならそのまま（数値は文字列化）
                id: match args.get("task_id") {
                    Some(Value::String(s)) if !s.is_empty() => s.clone(),
                    Some(v @ Value::Number(_)) if crate::types::is_truthy(v) => v.to_string(),
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

/// エージェントを MCP サーバーとして起動する（バイナリのエントリーポイント用）
pub async fn run_stdio(agent: Arc<dyn Subagent>, label: &str) {
    if let Err(e) = McpServer::new(agent).start().await {
        eprintln!("Failed to start {label} MCP Server: {e}");
        std::process::exit(1);
    }
}
