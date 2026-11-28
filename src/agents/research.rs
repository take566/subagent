//! Research Agent 実装

use crate::agents::common;
use crate::core::call::CallCtx;
use crate::core::subagent::{AgentBase, Subagent};
use crate::core::tool::Tool;
use crate::error::Result;
use crate::llm::{AgentKind, AgentLlm, AgentLlmSettings, LlmClient};
use crate::tools::{DocumentParserTool, WebFetchTool, WebSearchTool, js_display};
use crate::types::{
    Backoff, BehaviorConfig, ErrorAction, ErrorHandlingStrategy, InterfaceConfig,
    ParameterDefinition, RetryPolicy, RoleConfig, SubagentConfig, Task, TaskResult,
    ToolAlternative, js_substring,
};
use crate::utils::{ErrorHandler, RetryManager};
use async_trait::async_trait;
use rmcp::model::Tool as McpTool;
use serde_json::{Value, json};
use std::sync::Arc;

mod llm;
mod output;

pub use output::{Citation, ResearchResult, SearchError, Source, UNTRUSTED_TAG};

pub struct ResearchAgent {
    base: AgentBase,
    retry_manager: RetryManager,
    error_handler: ErrorHandler,
    llm: AgentLlm,
}

impl ResearchAgent {
    /// typed tool の名前
    pub const TYPED_TOOL: &'static str = "research";

    /// LLM を差し替える（`new()` などの既定は通信しない [`crate::llm::UnconfiguredLlm`]）
    pub fn with_llm(mut self, client: Arc<dyn LlmClient>, settings: AgentLlmSettings) -> Self {
        self.llm = AgentLlm::new(client, settings);
        self
    }

    pub fn llm(&self) -> &AgentLlm {
        &self.llm
    }

    pub fn default_config() -> SubagentConfig {
        SubagentConfig {
            id: "research-001".into(),
            name: "ResearchAgent".into(),
            version: "1.0.0".into(),
            role: RoleConfig {
                description: "情報収集と要約を担当".into(),
                capabilities: vec![
                    "web_search".into(),
                    "document_analysis".into(),
                    "summarization".into(),
                ],
                constraints: json!({ "max_search_queries": 10, "max_tokens_output": 4000 })
                    .as_object()
                    .cloned(),
            },
            interface: InterfaceConfig {
                input: vec![
                    ParameterDefinition::new("query", "string", true),
                    ParameterDefinition::new("context", "object", false),
                ],
                output: vec![ParameterDefinition {
                    schema: Some(
                        json!({ "summary": "string", "sources": "array", "confidence": "number" }),
                    ),
                    ..ParameterDefinition::new("result", "object", true)
                }],
            },
            tools: vec![
                "web_search".into(),
                "web_fetch".into(),
                "document_parser".into(),
            ],
            behavior: BehaviorConfig {
                retry_policy: Some(RetryPolicy {
                    max_attempts: 3,
                    backoff: Backoff::Exponential,
                    initial_delay_ms: Some(1000),
                }),
                // 打ち切りは SUBAGENT_DEADLINE_SECS（MCP）。Some にすると deadline をそれ以下に縮める
                timeout: None,
                fallback: Some("return_partial".into()),
            },
        }
    }

    pub fn new() -> Self {
        Self::with_config(Self::default_config())
    }

    pub fn with_config(config: SubagentConfig) -> Self {
        Self::with_tools(config, Self::default_tools())
    }

    /// 既定のツール（web_search / web_fetch / document_parser）
    pub fn default_tools() -> Vec<Arc<dyn Tool>> {
        vec![
            Arc::new(WebSearchTool::default()),
            Arc::new(WebFetchTool::default()),
            Arc::new(DocumentParserTool::default()),
        ]
    }

    /// ツールを差し替えて作る（テストでモックツールを渡す用）
    pub fn with_tools(config: SubagentConfig, tools: Vec<Arc<dyn Tool>>) -> Self {
        let policy = config
            .behavior
            .retry_policy
            .clone()
            .or_else(|| Self::default_config().behavior.retry_policy)
            .expect("default retry policy");
        let base = AgentBase::new(config, tools);
        let retry_manager = RetryManager::new(policy, base.logger.clone());
        let error_handler = ErrorHandler::new(
            ErrorHandlingStrategy {
                timeout: Some(ErrorAction {
                    max_retries: Some(3),
                    fallback: Some("delegate_to_alternative".into()),
                    ..ErrorAction::new("retry_with_backoff")
                }),
                tool_failure: Some(ErrorAction {
                    alternatives: Some(vec![ToolAlternative {
                        primary: "web_search".into(),
                        backup: "document_parser".into(),
                    }]),
                    ..ErrorAction::new("use_alternative_tool")
                }),
                partial_failure: Some(ErrorAction {
                    notify: Some(true),
                    ..ErrorAction::new("return_partial_with_flag")
                }),
                ..Default::default()
            },
            base.logger.clone(),
        );
        ResearchAgent {
            base,
            retry_manager,
            error_handler,
            llm: AgentLlm::unconfigured(AgentKind::Research),
        }
    }

    /// 情報を統合して要約を生成（実際の実装では LLM を使用）
    fn synthesize(details: &[Value], query: Option<&Value>) -> String {
        let summaries = details
            .iter()
            .map(|d| {
                [d.get("content"), d.get("summary")]
                    .into_iter()
                    .flatten()
                    .find(|v| crate::types::is_truthy(v))
                    .map(|v| js_display(Some(v)))
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        format!(
            "「{}」に関する調査結果:\n\n{}...",
            js_display(query),
            js_substring(&summaries, 1000)
        )
    }

    /// 信頼度を計算
    fn calculate_confidence(details: &[Value]) -> f64 {
        match details.len() {
            0 => 0.0,
            1 => 0.5,
            2 => 0.7,
            _ => 0.9,
        }
    }
}

impl Default for ResearchAgent {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Subagent for ResearchAgent {
    fn base(&self) -> &AgentBase {
        &self.base
    }

    fn typed_tool(&self) -> Option<McpTool> {
        Some(common::typed_tool(
            Self::TYPED_TOOL,
            "Web を調べて要約と出典を返す",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "調べる内容" },
                    "depth": {
                        "type": "string",
                        "enum": ["quick", "standard", "detailed"],
                        "description": "調べる深さ（既定 standard）"
                    },
                    "allowed_domains": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "検索・取得を許すドメイン（省略時は制限なし）"
                    }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
        ))
    }

    /// LLM（web_search / web_fetch の server tool）で調べ、[`ResearchResult`] を返す。
    /// LLM が未設定なら送らずに [`crate::llm::LlmError::NotConfigured`]
    async fn call_typed(&self, args: Value, ctx: CallCtx) -> Result<Value> {
        self.llm.ensure_configured()?;
        let args = llm::Args::parse(&args)?;
        let result = llm::research(&self.llm, &args, ctx).await?;
        Ok(serde_json::to_value(result)?)
    }

    async fn execute(&self, task: Task) -> Result<TaskResult> {
        let base = &self.base;
        base.run_task(&task, &self.error_handler, "RESEARCH_ERROR", || async {
            let query = task.parameters.get("query").cloned();
            let depth = task.param_str("depth").unwrap_or("standard");

            base.report_progress(&task.id, 0.1, "検索を開始");

            // 1. 検索実行
            let mut search_params = crate::types::JsonMap::new();
            if let Some(q) = &query {
                search_params.insert("query".into(), q.clone());
            }
            search_params.insert(
                "max_results".into(),
                json!(if depth == "detailed" { 10 } else { 5 }),
            );
            let search_results = self
                .retry_manager
                .execute_with_retry(|| base.call_tool("web_search", search_params.clone()))
                .await?;
            base.report_progress(&task.id, 0.3, "検索完了");

            // 2. 詳細取得（上位3件を並列で）
            let sources = search_results.get("results").cloned().unwrap_or(json!([]));
            let top: Vec<Value> = sources
                .as_array()
                .map(|a| a.iter().take(3).cloned().collect())
                .unwrap_or_default();
            let details = futures::future::try_join_all(top.iter().map(|r| {
                let mut p = crate::types::JsonMap::new();
                if let Some(url) = r.get("url") {
                    p.insert("url".into(), url.clone());
                }
                self.retry_manager
                    .execute_with_retry(move || base.call_tool("web_fetch", p.clone()))
            }))
            .await?;
            base.report_progress(&task.id, 0.7, "詳細取得完了");

            // 3. 要約生成
            let summary = Self::synthesize(&details, query.as_ref());
            base.report_progress(&task.id, 0.9, "要約生成完了");

            let confidence = Self::calculate_confidence(&details);
            let result = json!({
                "summary": summary,
                "sources": sources,
                "confidence": crate::types::js_number(confidence),
            });
            Ok((result, confidence, 1 + details.len() as u64))
        })
        .await
    }
}
