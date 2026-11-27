//! Review Agent 実装

use crate::core::subagent::{AgentBase, Subagent};
use crate::core::tool::Tool;
use crate::error::{Error, Result};
use crate::tools::{LintTool, ViewTool};
use crate::types::{
    Backoff, BehaviorConfig, ErrorAction, ErrorHandlingStrategy, InterfaceConfig, JsonMap,
    ParameterDefinition, RetryPolicy, RoleConfig, SubagentConfig, Task, TaskResult, js_len,
};
use crate::utils::{ErrorHandler, RetryManager};
use async_trait::async_trait;
use regex::Regex;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::{Arc, LazyLock};

pub struct ReviewAgent {
    base: AgentBase,
    #[allow(dead_code)] // 将来の拡張用（TS 版と同じ）
    retry_manager: RetryManager,
    error_handler: ErrorHandler,
}

struct Analysis {
    issues: Vec<Value>,
    suggestions: Vec<String>,
}

static COMMENT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"//|/\*|\*").unwrap());

impl ReviewAgent {
    pub fn default_config() -> SubagentConfig {
        SubagentConfig {
            id: "review-001".into(),
            name: "ReviewAgent".into(),
            version: "1.0.0".into(),
            role: RoleConfig {
                description: "コードレビュー・品質チェック".into(),
                capabilities: vec![
                    "code_analysis".into(),
                    "quality_check".into(),
                    "security_scan".into(),
                ],
                constraints: json!({ "max_file_size": 500000 }).as_object().cloned(),
            },
            interface: InterfaceConfig {
                input: vec![
                    ParameterDefinition::new("source_code", "string", true),
                    ParameterDefinition::new("criteria", "object", false),
                    ParameterDefinition::new("lint_command", "string", false),
                ],
                output: vec![ParameterDefinition {
                    schema: Some(
                        json!({ "issues": "array", "suggestions": "array", "score": "number" }),
                    ),
                    ..ParameterDefinition::new("result", "object", true)
                }],
            },
            tools: vec!["view".into(), "bash_tool".into()],
            behavior: BehaviorConfig {
                retry_policy: Some(RetryPolicy {
                    max_attempts: 2,
                    backoff: Backoff::Linear,
                    initial_delay_ms: Some(500),
                }),
                timeout: Some(60000),
                fallback: Some("return_partial".into()),
            },
        }
    }

    pub fn new() -> Self {
        Self::with_config(Self::default_config())
    }

    pub fn with_config(config: SubagentConfig) -> Self {
        let policy = config
            .behavior
            .retry_policy
            .clone()
            .or_else(|| Self::default_config().behavior.retry_policy)
            .expect("default retry policy");
        let tools: Vec<Arc<dyn Tool>> =
            vec![Arc::new(ViewTool::default()), Arc::new(LintTool::default())];
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
        ReviewAgent {
            base,
            retry_manager,
            error_handler,
        }
    }

    /// `source_code` がファイルパスか（TS は `/` 始まりのみ。Windows の絶対パスも扱う）
    fn is_file_path(s: &str) -> bool {
        s.starts_with('/') || Path::new(s).is_absolute()
    }

    /// コードを分析
    fn analyze_code(code: &str) -> Analysis {
        let mut issues = vec![];
        let mut suggestions = vec![];

        if js_len(code) > 10000 {
            issues.push(json!({
                "type": "warning",
                "message": "ファイルサイズが大きすぎます。分割を検討してください。",
                "severity": "medium",
            }));
        }

        let comment_ratio =
            COMMENT_RE.find_iter(code).count() as f64 / code.split('\n').count() as f64;
        if comment_ratio < 0.1 {
            suggestions.push("コメントを追加してコードの可読性を向上させてください。".into());
        }

        Analysis {
            issues,
            suggestions,
        }
    }

    /// スコアを計算（0〜100 の整数）
    fn calculate_score(issues: &[Value], analysis: &Analysis) -> i64 {
        let count = |t: &str| {
            issues
                .iter()
                .filter(|i| i.get("type") == Some(&json!(t)))
                .count() as i64
        };
        let score =
            100 - count("error") * 10 - count("warning") * 2 - analysis.suggestions.len() as i64;
        score.clamp(0, 100)
    }
}

impl Default for ReviewAgent {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Subagent for ReviewAgent {
    fn base(&self) -> &AgentBase {
        &self.base
    }

    async fn execute(&self, task: Task) -> Result<TaskResult> {
        let base = &self.base;
        base.run_task(&task, &self.error_handler, "REVIEW_ERROR", || async {
            let source = task.parameters.get("source_code");
            let mut tools_called = 0u64;

            base.report_progress(&task.id, 0.2, "コードレビューを開始");

            // 1. コードを読み込み
            // TS 版は文字列以外（未指定を含む）で TypeError → failed になる
            let Some(Value::String(source)) = source else {
                return Err(Error::InvalidParameter(
                    "`source_code` (string) is required".into(),
                ));
            };
            let code = if Self::is_file_path(source) {
                let mut p = JsonMap::new();
                p.insert("path".into(), json!(source));
                let info = base.call_tool("view", p).await?;
                tools_called += 1;
                info.get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            } else {
                source.clone()
            };
            base.report_progress(&task.id, 0.4, "コード読み込み完了");

            // 2. Lint 実行。TS 版は `npm run lint` を固定で実行していたが、
            //    npm プロジェクトでなくなったため `lint_command` 指定時のみ実行する。
            let mut lint_issues: Vec<Value> = vec![];
            if let Some(cmd) = task.param_str("lint_command") {
                let mut p = JsonMap::new();
                p.insert("command".into(), json!(cmd));
                if let Some(cwd) = task.param_str("cwd") {
                    p.insert("cwd".into(), json!(cwd));
                }
                tools_called += 1;
                match base.call_tool("bash_tool", p).await {
                    Ok(r) => {
                        if let Some(Value::Array(issues)) = r.get("issues") {
                            lint_issues = issues.clone();
                        }
                    }
                    Err(e) => base.logger.warn(
                        "Lint実行に失敗しました",
                        Some(&json!({ "error": e.to_string() })),
                    ),
                }
            }
            base.report_progress(&task.id, 0.6, "Lint実行完了");

            // 3. コード分析
            let analysis = Self::analyze_code(&code);
            base.report_progress(&task.id, 0.8, "コード分析完了");

            // 4. レビュー結果を統合
            let mut issues = lint_issues;
            issues.extend(analysis.issues.iter().cloned());
            let score = Self::calculate_score(&issues, &analysis);
            base.report_progress(&task.id, 0.9, "レビュー完了");

            Ok((
                json!({ "issues": issues, "suggestions": analysis.suggestions, "score": score }),
                0.9,
                tools_called,
            ))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analysis_and_score() {
        let a = ReviewAgent::analyze_code("fn a() {}\nfn b() {}");
        assert_eq!(a.suggestions.len(), 1);
        assert_eq!(ReviewAgent::calculate_score(&[], &a), 99);

        let a = ReviewAgent::analyze_code("/* doc */\nfn a() {}");
        assert!(a.suggestions.is_empty());

        let issues = vec![json!({"type": "error"}), json!({"type": "warning"})];
        assert_eq!(ReviewAgent::calculate_score(&issues, &a), 88);
        let many: Vec<Value> = (0..20).map(|_| json!({"type": "error"})).collect();
        assert_eq!(ReviewAgent::calculate_score(&many, &a), 0);
    }

    #[test]
    fn file_path_detection() {
        assert!(ReviewAgent::is_file_path("/src/a.rs"));
        assert!(!ReviewAgent::is_file_path("fn main() {}"));
        #[cfg(windows)]
        assert!(ReviewAgent::is_file_path(r"C:\src\a.rs"));
    }
}
