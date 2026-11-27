//! Document Agent 実装

use crate::core::subagent::{AgentBase, Subagent};
use crate::core::tool::Tool;
use crate::error::{Error, Result};
use crate::tools::FileCreateTool;
use crate::types::{
    now_iso, Backoff, BehaviorConfig, ErrorAction, ErrorHandlingStrategy, InterfaceConfig,
    JsonMap, ParameterDefinition, RetryPolicy, RoleConfig, SubagentConfig, Task, TaskResult,
};
use crate::utils::{ErrorHandler, RetryManager};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

pub struct DocumentAgent {
    base: AgentBase,
    #[allow(dead_code)] // 将来の拡張用（TS 版と同じ）
    retry_manager: RetryManager,
    error_handler: ErrorHandler,
}

impl DocumentAgent {
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
            tools: vec!["file_create".into()],
            behavior: BehaviorConfig {
                retry_policy: Some(RetryPolicy {
                    max_attempts: 2,
                    backoff: Backoff::Fixed,
                    initial_delay_ms: Some(500),
                }),
                timeout: Some(30000),
                fallback: Some("return_partial".into()),
            },
        }
    }

    /// カレントディレクトリの `docs/` に出力するエージェント
    pub fn new() -> Self {
        Self::with_options(Self::default_config(), None)
    }

    /// `base_dir` を指定すると `<base_dir>/docs/` に出力する
    pub fn with_options(config: SubagentConfig, base_dir: Option<PathBuf>) -> Self {
        let policy = config
            .behavior
            .retry_policy
            .clone()
            .or_else(|| Self::default_config().behavior.retry_policy)
            .expect("default retry policy");
        let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(FileCreateTool::new(base_dir))];
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
