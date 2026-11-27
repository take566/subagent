//! エラーハンドリング実装

use crate::error::Error;
use crate::types::{ErrorHandlingStrategy, ErrorInfo, TaskResult};
use crate::utils::logger::Logger;
use serde_json::json;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeoutDecision {
    pub should_retry: bool,
    pub action: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationDecision {
    pub action: String,
    pub should_escalate: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolFailureDecision {
    pub use_alternative: bool,
    pub alternative_tool: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartialFailureDecision {
    pub return_partial: bool,
    pub notify: bool,
}

#[derive(Debug, Clone)]
pub struct ErrorHandler {
    strategies: ErrorHandlingStrategy,
    logger: Logger,
}

impl ErrorHandler {
    pub fn new(strategies: ErrorHandlingStrategy, logger: Logger) -> Self {
        ErrorHandler { strategies, logger }
    }

    /// タイムアウトエラーを処理（TS 版の既定 `max_retries = 3`）
    pub fn handle_timeout(
        &self,
        _error: &Error,
        retry_count: u32,
        max_retries: u32,
    ) -> TimeoutDecision {
        let Some(strategy) = &self.strategies.timeout else {
            return TimeoutDecision {
                should_retry: false,
                action: "fail".into(),
            };
        };
        if retry_count >= max_retries {
            self.logger.warn(
                "Max retries reached for timeout",
                Some(&json!({ "retryCount": retry_count })),
            );
            return TimeoutDecision {
                should_retry: false,
                action: strategy.fallback.clone().unwrap_or_else(|| "fail".into()),
            };
        }
        TimeoutDecision {
            should_retry: true,
            action: strategy.action.clone(),
        }
    }

    /// バリデーションエラーを処理
    pub fn handle_validation_error(
        &self,
        _error: &Error,
        attempt_count: u32,
    ) -> ValidationDecision {
        let Some(strategy) = &self.strategies.validation_error else {
            return ValidationDecision {
                action: "fail".into(),
                should_escalate: false,
            };
        };
        // TS: escalate_after が 0/未設定なら false
        let should_escalate = match strategy.escalate_after {
            Some(n) if n > 0 => attempt_count >= n,
            _ => false,
        };
        ValidationDecision {
            action: strategy.action.clone(),
            should_escalate,
        }
    }

    /// ツール失敗を処理
    pub fn handle_tool_failure(&self, tool_name: &str, _error: &Error) -> ToolFailureDecision {
        let none = ToolFailureDecision {
            use_alternative: false,
            alternative_tool: None,
        };
        let Some(alternatives) = self
            .strategies
            .tool_failure
            .as_ref()
            .and_then(|s| s.alternatives.as_ref())
        else {
            return none;
        };
        match alternatives.iter().find(|a| a.primary == tool_name) {
            Some(alt) => {
                self.logger.info(
                    &format!("Switching from {tool_name} to {}", alt.backup),
                    None,
                );
                ToolFailureDecision {
                    use_alternative: true,
                    alternative_tool: Some(alt.backup.clone()),
                }
            }
            None => none,
        }
    }

    /// 部分的な失敗を処理
    pub fn handle_partial_failure(
        &self,
        _result: &TaskResult,
        _error: &Error,
    ) -> PartialFailureDecision {
        match &self.strategies.partial_failure {
            None => PartialFailureDecision {
                return_partial: false,
                notify: false,
            },
            Some(s) => PartialFailureDecision {
                return_partial: s.action == "return_partial_with_flag",
                notify: s.notify.unwrap_or(false),
            },
        }
    }

    /// エラー情報を作成（Rust にはスタックトレースが無いので stack は None）
    pub fn create_error_info(&self, error: &Error, code: &str) -> ErrorInfo {
        ErrorInfo {
            code: code.into(),
            message: error.to_string(),
            details: Some(json!({ "name": error.name() })),
            stack: None,
        }
    }

    /// エスカレーションが必要か判定
    pub fn should_escalate(&self, result: &TaskResult, conditions: &[&str]) -> bool {
        for condition in conditions {
            match *condition {
                // TS: `(result.confidence || 1.0) < 0.5` — 0 や未設定は 1.0 扱い
                "confidence < 0.5" => {
                    let c = result.confidence.filter(|c| *c != 0.0).unwrap_or(1.0);
                    if c < 0.5 {
                        return true;
                    }
                }
                "critical_error" if result.error.is_some() => return true,
                // TS 版は requires_approval で即 false を返す（以降の条件を見ない）
                "requires_approval" => return false,
                _ => {}
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ErrorAction, TaskStatus, ToolAlternative};
    use crate::utils::logger::LogLevel;

    fn handler(s: ErrorHandlingStrategy) -> ErrorHandler {
        ErrorHandler::new(s, Logger::with_level(None, LogLevel::Error))
    }

    fn result(confidence: Option<f64>, error: bool) -> TaskResult {
        TaskResult {
            task_id: "t".into(),
            status: TaskStatus::Completed,
            result: None,
            error: error.then(|| ErrorInfo {
                code: "X".into(),
                message: "m".into(),
                details: None,
                stack: None,
            }),
            confidence,
            metrics: None,
        }
    }

    #[test]
    fn timeout() {
        let e = Error::other("t");
        assert!(
            !handler(Default::default())
                .handle_timeout(&e, 0, 3)
                .should_retry
        );
        let h = handler(ErrorHandlingStrategy {
            timeout: Some(ErrorAction {
                fallback: Some("delegate".into()),
                ..ErrorAction::new("retry_with_backoff")
            }),
            ..Default::default()
        });
        assert_eq!(
            h.handle_timeout(&e, 1, 3),
            TimeoutDecision {
                should_retry: true,
                action: "retry_with_backoff".into()
            }
        );
        assert_eq!(
            h.handle_timeout(&e, 3, 3),
            TimeoutDecision {
                should_retry: false,
                action: "delegate".into()
            }
        );
    }

    #[test]
    fn validation_and_tool_failure() {
        let e = Error::other("v");
        let h = handler(ErrorHandlingStrategy {
            validation_error: Some(ErrorAction {
                escalate_after: Some(2),
                ..ErrorAction::new("request_clarification")
            }),
            tool_failure: Some(ErrorAction {
                alternatives: Some(vec![ToolAlternative {
                    primary: "web_search".into(),
                    backup: "document_parser".into(),
                }]),
                ..ErrorAction::new("use_alternative_tool")
            }),
            ..Default::default()
        });
        assert!(!h.handle_validation_error(&e, 1).should_escalate);
        assert!(h.handle_validation_error(&e, 2).should_escalate);
        assert_eq!(
            h.handle_tool_failure("web_search", &e)
                .alternative_tool
                .as_deref(),
            Some("document_parser")
        );
        assert!(!h.handle_tool_failure("web_fetch", &e).use_alternative);
    }

    #[test]
    fn partial_failure() {
        let e = Error::other("p");
        let h = handler(ErrorHandlingStrategy {
            partial_failure: Some(ErrorAction {
                notify: Some(true),
                ..ErrorAction::new("return_partial_with_flag")
            }),
            ..Default::default()
        });
        assert_eq!(
            h.handle_partial_failure(&result(None, false), &e),
            PartialFailureDecision {
                return_partial: true,
                notify: true
            }
        );
    }

    #[test]
    fn escalation() {
        let h = handler(Default::default());
        assert!(h.should_escalate(&result(Some(0.3), false), &["confidence < 0.5"]));
        assert!(!h.should_escalate(&result(Some(0.0), false), &["confidence < 0.5"]));
        assert!(!h.should_escalate(&result(None, false), &["confidence < 0.5"]));
        assert!(h.should_escalate(&result(None, true), &["critical_error"]));
        assert!(!h.should_escalate(
            &result(None, true),
            &["requires_approval", "critical_error"]
        ));
    }

    #[test]
    fn error_info() {
        let h = handler(Default::default());
        let info = h.create_error_info(&Error::NoSubagent("x".into()), "CODE");
        assert_eq!(info.code, "CODE");
        assert_eq!(info.message, "No suitable subagent found for action: x");
    }
}
