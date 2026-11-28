//! Document Agent 実装

use crate::agents::common;
use crate::core::call::CallCtx;
use crate::core::subagent::{AgentBase, Subagent};
use crate::core::tool::Tool;
use crate::error::{Error, Result};
use crate::llm::{AgentKind, AgentLlm, AgentLlmSettings, LlmClient};
use crate::sandbox::Sandbox;
use crate::tools::{FileCreateTool, NotionClient, NotionPageCreateTool, NotionPageUpdateTool};
use crate::types::{
    Backoff, BehaviorConfig, ErrorAction, ErrorHandlingStrategy, InterfaceConfig, JsonMap,
    ParameterDefinition, RetryPolicy, RoleConfig, SubagentConfig, Task, TaskResult, now_iso,
};
use crate::utils::{ErrorHandler, RetryManager};
use async_trait::async_trait;
use rmcp::model::Tool as McpTool;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

pub struct DocumentAgent {
    base: AgentBase,
    #[allow(dead_code)] // 将来の拡張用（TS 版と同じ）
    retry_manager: RetryManager,
    error_handler: ErrorHandler,
    llm: AgentLlm,
}

impl DocumentAgent {
    /// typed tool の名前
    pub const TYPED_TOOL: &'static str = "document";

    /// LLM を差し替える（`new()` などの既定は通信しない [`crate::llm::UnconfiguredLlm`]）
    pub fn with_llm(mut self, client: Arc<dyn LlmClient>, settings: AgentLlmSettings) -> Self {
        self.llm = AgentLlm::new(client, settings);
        self
    }

    pub fn llm(&self) -> &AgentLlm {
        &self.llm
    }

    /// typed tool の引数を `execute` の parameters に写す（format の `md` は `markdown`）
    fn typed_params(args: &Value) -> JsonMap {
        let mut p = JsonMap::new();
        common::copy_arg(args, "content", &mut p, "content");
        if let Some(format) = args.get("format").and_then(Value::as_str) {
            let format = if format == "md" { "markdown" } else { format };
            p.insert("format".into(), json!(format));
        }
        p
    }

    pub fn default_config() -> SubagentConfig {
        SubagentConfig {
            id: "document-001".into(),
            name: "DocumentAgent".into(),
            version: "1.0.0".into(),
            role: RoleConfig {
                description: "ドキュメント作成・編集".into(),
                capabilities: vec![
                    "document_creation".into(),
                    "formatting".into(),
                    "translation".into(),
                ],
                constraints: json!({ "max_document_size": 1000000 }).as_object().cloned(),
            },
            interface: InterfaceConfig {
                input: vec![
                    ParameterDefinition::new("content", "string", true),
                    ParameterDefinition::new("format", "string", false),
                ],
                output: vec![ParameterDefinition {
                    schema: Some(json!({ "document_path": "string", "format": "string" })),
                    ..ParameterDefinition::new("result", "object", true)
                }],
            },
            tools: vec![
                "file_create".into(),
                "notion_page_create".into(),
                "notion_page_update".into(),
            ],
            behavior: BehaviorConfig {
                retry_policy: Some(RetryPolicy {
                    max_attempts: 2,
                    backoff: Backoff::Fixed,
                    initial_delay_ms: Some(500),
                }),
                // 打ち切りは SUBAGENT_DEADLINE_SECS（MCP）。Some にすると deadline をそれ以下に縮める
                timeout: None,
                fallback: Some("return_partial".into()),
            },
        }
    }

    /// `SUBAGENT_ROOTS` の workspace の `docs/` に出力するエージェント（未設定ならツールが拒否する）
    pub fn new() -> Self {
        Self::with_options(Self::default_config(), None)
    }

    /// `base_dir` を指定すると `<base_dir>/docs/` に出力する（None は `SUBAGENT_ROOTS`）
    pub fn with_options(config: SubagentConfig, base_dir: Option<PathBuf>) -> Self {
        Self::with_tools(config, Self::default_tools(base_dir))
    }

    /// 既定のツール（file_create / notion_page_create / notion_page_update）。
    /// `file_create` は `base_dir`（None は `SUBAGENT_ROOTS`）を workspace root にする。
    /// Notion ツールは実 API と環境変数 `NOTION_TOKEN`（呼び出し時に読む）を使う。
    pub fn default_tools(base_dir: Option<PathBuf>) -> Vec<Arc<dyn Tool>> {
        let sandbox = match base_dir {
            Some(dir) => Sandbox::with_root(dir),
            None => Sandbox::from_env(),
        };
        let notion = NotionClient::from_env();
        vec![
            Arc::new(FileCreateTool::new(Arc::new(sandbox))),
            Arc::new(NotionPageCreateTool::new(notion.clone())),
            Arc::new(NotionPageUpdateTool::new(notion)),
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
                validation_error: Some(ErrorAction {
                    escalate_after: Some(1),
                    ..ErrorAction::new("request_clarification")
                }),
                ..Default::default()
            },
            base.logger.clone(),
        );
        DocumentAgent {
            base,
            retry_manager,
            error_handler,
            llm: AgentLlm::unconfigured(AgentKind::Document),
        }
    }

    /// フォーマットに応じてコンテンツを変換
    fn format_content(content: &str, format: &str) -> String {
        match format.to_lowercase().as_str() {
            // 実際の実装では MarkdownからHTMLへの変換などを行う
            "html" => format!("<html><body><pre>{content}</pre></body></html>"),
            _ => content.to_string(),
        }
    }

    /// ファイルパスを生成
    fn generate_file_path(format: &str) -> String {
        let ts = now_iso().replace([':', '.'], "-");
        let ext = match format {
            "markdown" => "md",
            "html" => "html",
            _ => "txt",
        };
        format!("docs/generated-{ts}.{ext}")
    }
}

impl Default for DocumentAgent {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Subagent for DocumentAgent {
    fn base(&self) -> &AgentBase {
        &self.base
    }

    fn typed_tool(&self) -> Option<McpTool> {
        Some(common::typed_tool(
            Self::TYPED_TOOL,
            "ドキュメントを書いて workspace の docs/ に保存する",
            json!({
                "type": "object",
                "properties": {
                    "content": { "type": "string", "description": "書く内容（verbatim ならそのまま保存する本文）" },
                    "format": {
                        "type": "string",
                        "enum": ["md", "txt", "html"],
                        "description": "出力形式（既定 md）"
                    },
                    "mode": {
                        "type": "string",
                        "enum": ["llm", "verbatim"],
                        "description": "llm は LLM が書く（既定）、verbatim は content をそのまま保存する"
                    },
                    "title": { "type": "string", "description": "題名" }
                },
                "required": ["content"],
                "additionalProperties": false
            }),
        ))
    }

    /// 3a の仮実装: 引数を既存の `execute` に写す（LLM が未設定ならエラー）。
    /// `mode` / `title` はまだ使わない。3b で置き換える
    async fn call_typed(&self, args: Value, _ctx: CallCtx) -> Result<Value> {
        let p = Self::typed_params(&args);
        self.llm.ensure_configured()?;
        common::task_value(self.execute(common::typed_task("document", p)).await?)
    }

    async fn execute(&self, task: Task) -> Result<TaskResult> {
        let base = &self.base;
        base.run_task(&task, &self.error_handler, "DOCUMENT_ERROR", || async {
            let content = task
                .param_str("content")
                .ok_or_else(|| Error::InvalidParameter("`content` (string) is required".into()))?;
            // TS は既定値が undefined のときだけ効き、null や数値では toLowerCase で失敗する
            let format = match task.parameters.get("format") {
                None => "markdown",
                Some(Value::String(s)) => s.as_str(),
                Some(_) => {
                    return Err(Error::InvalidParameter("`format` must be a string".into()));
                }
            };

            base.report_progress(&task.id, 0.2, "ドキュメント作成を開始");

            // 1. フォーマット変換
            let formatted = Self::format_content(content, format);
            base.report_progress(&task.id, 0.5, "フォーマット変換完了");

            // 2. ファイルパスを決定
            let path = Self::generate_file_path(format);
            base.report_progress(&task.id, 0.7, "ファイルパス決定");

            // 3. ファイル作成
            let mut p = JsonMap::new();
            p.insert("path".into(), json!(path));
            p.insert("content".into(), json!(formatted));
            let result = base.call_tool("file_create", p).await?;
            base.report_progress(&task.id, 0.9, "ファイル作成完了");

            Ok((
                json!({ "document_path": result["path"], "format": format }),
                0.95,
                1,
            ))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_format_md_maps_to_markdown() {
        let p = |args: Value| Value::Object(DocumentAgent::typed_params(&args));
        assert_eq!(
            p(json!({ "content": "c", "format": "md", "mode": "llm", "title": "t" })),
            json!({ "content": "c", "format": "markdown" })
        );
        assert_eq!(
            p(json!({ "content": "c", "format": "html" })),
            json!({ "content": "c", "format": "html" })
        );
        // 省略時は execute の既定（markdown）
        assert_eq!(p(json!({ "content": "c" })), json!({ "content": "c" }));
    }

    #[test]
    fn formatting_and_paths() {
        assert_eq!(
            DocumentAgent::format_content("x", "HTML"),
            "<html><body><pre>x</pre></body></html>"
        );
        assert_eq!(DocumentAgent::format_content("x", "md"), "x");
        let p = DocumentAgent::generate_file_path("markdown");
        assert!(p.starts_with("docs/generated-") && p.ends_with(".md"));
        assert!(!p.contains(':'));
        // TS と同じく拡張子判定は小文字化しない
        assert!(DocumentAgent::generate_file_path("HTML").ends_with(".txt"));
    }
}
