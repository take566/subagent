//! Code Generation Agent 実装

use crate::agents::common;
use crate::core::call::CallCtx;
use crate::core::subagent::{AgentBase, Subagent};
use crate::core::tool::Tool;
use crate::error::Result;
use crate::llm::{AgentKind, AgentLlm, AgentLlmSettings, LlmClient};
use crate::sandbox::Sandbox;
use crate::tools::{BashTool, FileCreateTool, StrReplaceTool, js_display};
use crate::types::{
    Backoff, BehaviorConfig, ErrorAction, ErrorHandlingStrategy, InterfaceConfig, JsonMap,
    ParameterDefinition, RetryPolicy, RoleConfig, SubagentConfig, Task, TaskResult,
};
use crate::utils::{ErrorHandler, RetryManager};
use async_trait::async_trait;
use rmcp::model::Tool as McpTool;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

pub struct CodeGenAgent {
    base: AgentBase,
    #[allow(dead_code)] // 将来の拡張用（TS 版と同じ）
    retry_manager: RetryManager,
    error_handler: ErrorHandler,
    llm: AgentLlm,
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
                // 打ち切りは SUBAGENT_DEADLINE_SECS（MCP）。Some にすると deadline をそれ以下に縮める
                timeout: None,
                fallback: Some("return_partial".into()),
            },
        }
    }

    /// `SUBAGENT_ROOTS` の workspace にファイルを生成するエージェント（未設定ならツールが拒否する）
    pub fn new() -> Self {
        Self::with_options(Self::default_config(), None)
    }

    /// `base_dir` を指定するとそこを workspace root にする（None は `SUBAGENT_ROOTS`）
    pub fn with_options(config: SubagentConfig, base_dir: Option<PathBuf>) -> Self {
        Self::with_tools(config, Self::default_tools(base_dir))
    }

    /// sandbox を指定して作る
    pub fn with_sandbox(config: SubagentConfig, sandbox: Arc<Sandbox>) -> Self {
        Self::with_tools(config, Self::default_tools_in(sandbox))
    }

    /// 既定のツール（file_create / bash_tool / str_replace）。
    /// `base_dir` を workspace root にする（None は env の `SUBAGENT_ROOTS`）
    pub fn default_tools(base_dir: Option<PathBuf>) -> Vec<Arc<dyn Tool>> {
        let sandbox = match base_dir {
            Some(dir) => Sandbox::with_root(dir),
            None => Sandbox::from_env(),
        };
        Self::default_tools_in(Arc::new(sandbox))
    }

    /// 既定のツールを、指定した sandbox で作る
    pub fn default_tools_in(sandbox: Arc<Sandbox>) -> Vec<Arc<dyn Tool>> {
        vec![
            Arc::new(FileCreateTool::new(sandbox.clone())),
            Arc::new(BashTool::new(sandbox.clone())),
            Arc::new(StrReplaceTool::new(sandbox)),
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
            llm: AgentLlm::unconfigured(AgentKind::CodeGen),
        }
    }

    /// typed tool の名前
    pub const TYPED_TOOL: &'static str = "codegen";

    /// LLM を差し替える（`new()` などの既定は通信しない [`crate::llm::UnconfiguredLlm`]）
    pub fn with_llm(mut self, client: Arc<dyn LlmClient>, settings: AgentLlmSettings) -> Self {
        self.llm = AgentLlm::new(client, settings);
        self
    }

    pub fn llm(&self) -> &AgentLlm {
        &self.llm
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

    fn typed_tool(&self) -> Option<McpTool> {
        Some(common::typed_tool(
            Self::TYPED_TOOL,
            "workspace（SUBAGENT_ROOTS）の中でコードを書く・直す",
            json!({
                "type": "object",
                "properties": {
                    "task": { "type": "string", "description": "作るもの・直すことの説明" },
                    "files": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "関係するファイル（workspace からの相対パス）"
                    },
                    "allow_commands": {
                        "type": "boolean",
                        "description": "allowlist（SUBAGENT_CODEGEN_ALLOWED_COMMANDS）のコマンドの実行を許すか（既定 false）"
                    }
                },
                "required": ["task"],
                "additionalProperties": false
            }),
        ))
    }

    /// 3a の仮実装: `task` を既存の `execute` の `specification` に写す（LLM が未設定ならエラー）。
    /// `files` / `allow_commands` はまだ使わない。3e で置き換える
    async fn call_typed(&self, args: Value, _ctx: CallCtx) -> Result<Value> {
        let mut p = JsonMap::new();
        common::copy_arg(&args, "task", &mut p, "specification");
        self.llm.ensure_configured()?;
        common::task_value(self.execute(common::typed_task("codegen", p)).await?)
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
