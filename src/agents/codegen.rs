//! Code Generation Agent 実装

use crate::core::subagent::{AgentBase, Subagent};
use crate::core::tool::Tool;
use crate::error::Result;
use crate::tools::{BashTool, FileCreateTool, StrReplaceTool, js_display};
use crate::types::{
    Backoff, BehaviorConfig, ErrorAction, ErrorHandlingStrategy, InterfaceConfig, JsonMap,
    ParameterDefinition, RetryPolicy, RoleConfig, SubagentConfig, Task, TaskResult,
};
use crate::utils::{ErrorHandler, RetryManager};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

pub struct CodeGenAgent {
    base: AgentBase,
    #[allow(dead_code)] // 将来の拡張用（TS 版と同じ）
    retry_manager: RetryManager,
    error_handler: ErrorHandler,
}

/// 生成結果（ファイルパス → 内容）
struct GeneratedCode {
    files: Vec<(String, String)>,
    tests: Option<Vec<(String, String)>>,
}

impl CodeGenAgent {
    pub fn default_config() -> SubagentConfig {
        SubagentConfig {
            id: "codegen-001".into(),
            name: "CodeGenAgent".into(),
            version: "1.0.0".into(),
            role: RoleConfig {
                description: "コード生成・リファクタリング".into(),
                capabilities: vec![
                    "code_generation".into(),
                    "refactoring".into(),
                    "test_generation".into(),
                ],
                constraints: json!({ "max_file_size": 100000, "max_files_per_task": 50 })
                    .as_object()
                    .cloned(),
            },
            interface: InterfaceConfig {
                input: vec![
                    ParameterDefinition::new("specification", "string", true),
                    ParameterDefinition::new("language", "string", false),
                    ParameterDefinition::new("framework", "string", false),
                ],
                output: vec![ParameterDefinition {
                    schema: Some(json!({ "files": "array", "test_files": "array" })),
                    ..ParameterDefinition::new("result", "object", true)
                }],
            },
            tools: vec![
                "file_create".into(),
                "bash_tool".into(),
                "str_replace".into(),
            ],
            behavior: BehaviorConfig {
                retry_policy: Some(RetryPolicy {
                    max_attempts: 3,
                    backoff: Backoff::Exponential,
                    initial_delay_ms: Some(1000),
                }),
                timeout: Some(120000),
                fallback: Some("return_partial".into()),
            },
        }
    }

    /// カレントディレクトリにファイルを生成するエージェント
    pub fn new() -> Self {
        Self::with_options(Self::default_config(), None)
    }

    /// `base_dir` を指定すると、相対パスはそこを基準に解決する
    pub fn with_options(config: SubagentConfig, base_dir: Option<PathBuf>) -> Self {
        Self::with_tools(config, Self::default_tools(base_dir))
    }

    /// 既定のツール（file_create / bash_tool / str_replace）。相対パスは `base_dir` 基準
    pub fn default_tools(base_dir: Option<PathBuf>) -> Vec<Arc<dyn Tool>> {
        vec![
            Arc::new(FileCreateTool::new(base_dir.clone())),
            Arc::new(BashTool::default()),
            Arc::new(StrReplaceTool::new(base_dir)),
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
                    escalate_after: Some(2),
                    ..ErrorAction::new("request_clarification")
                }),
                tool_failure: Some(ErrorAction::new("use_alternative_tool")),
                ..Default::default()
            },
            base.logger.clone(),
        );
        CodeGenAgent {
            base,
            retry_manager,
            error_handler,
        }
    }

    /// コードを生成（モック実装。実際の実装では LLM を使用）
    fn generate_code(specification: Option<&Value>, language: &str) -> GeneratedCode {
        let ext = if language == "typescript" { "ts" } else { "js" };
        let file_name = format!("generated.{ext}");
        let code = format!(
            "// Generated code for: {}\n\nexport function main() {{\n  // Implementation here\n}}\n",
            js_display(specification)
        );
        GeneratedCode {
            tests: Some(vec![(
                format!("{file_name}.test.{ext}"),
                "// Test code\n".into(),
            )]),
            files: vec![(file_name, code)],
        }
    }
}

impl Default for CodeGenAgent {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Subagent for CodeGenAgent {
    fn base(&self) -> &AgentBase {
        &self.base
    }

    async fn execute(&self, task: Task) -> Result<TaskResult> {
        let base = &self.base;
        base.run_task(&task, &self.error_handler, "CODEGEN_ERROR", || async {
            let specification = task.parameters.get("specification");
            // TS の分割代入の既定値は undefined のときだけ効く（null や数値なら .js）
            let language = match task.parameters.get("language") {
                None => "typescript",
                Some(v) => v.as_str().unwrap_or(""),
            };

            base.report_progress(&task.id, 0.2, "コード生成を開始");

            // 1. コード生成
            let generated = Self::generate_code(specification, language);
            base.report_progress(&task.id, 0.5, "コード生成完了");

            let create = |path: String, content: String| {
                let mut p = JsonMap::new();
                p.insert("path".into(), json!(path));
                p.insert("content".into(), json!(content));
                base.call_tool("file_create", p)
            };

            // 2. ファイル作成
            let mut files = vec![];
            for (path, content) in generated.files {
                files.push(create(path, content).await?);
            }
            base.report_progress(&task.id, 0.7, "ファイル作成完了");

            // 3. テストコード生成
            let mut test_files = vec![];
            for (path, content) in generated.tests.unwrap_or_default() {
                test_files.push(create(path, content).await?);
            }
            base.report_progress(&task.id, 0.9, "テストコード生成完了");

            let tools_called = (files.len() + test_files.len()) as u64;
            Ok((
                json!({ "files": files, "test_files": test_files }),
                0.85,
                tools_called,
            ))
        })
        .await
    }
}
