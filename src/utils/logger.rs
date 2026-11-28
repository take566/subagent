//! ロガー実装
//!
//! 構造化ログ（JSON）を **stderr** に出力する。MCP の stdio トランスポートは
//! stdout を JSON-RPC 専用に使うため、ログを stdout に出してはいけない。
//!
//! TS 版 #44 と同じく、[`Logger::child`] で task_id / correlation_id を固定した子ロガーを作り、
//! 各ログ呼び出しで [`LogFields`]（duration / status）を付けられる。

use serde::Serialize;
use serde_json::Value;
use std::io::Write;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Debug = 0,
    Info = 1,
    Warn = 2,
    Error = 3,
}

impl LogLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
        }
    }

    /// `SUBAGENT_LOG_LEVEL` 環境変数（debug/info/warn/error/off）から既定レベルを決める
    fn from_env() -> Option<LogLevel> {
        match std::env::var("SUBAGENT_LOG_LEVEL")
            .ok()?
            .to_ascii_lowercase()
            .as_str()
        {
            "debug" => Some(LogLevel::Debug),
            "info" => Some(LogLevel::Info),
            "warn" => Some(LogLevel::Warn),
            "error" => Some(LogLevel::Error),
            _ => None,
        }
    }

    fn logging_disabled() -> bool {
        std::env::var("SUBAGENT_LOG_LEVEL").is_ok_and(|v| v.eq_ignore_ascii_case("off"))
    }
}

#[derive(Debug, Serialize)]
pub struct LogEntry<'a> {
    pub timestamp: String,
    pub level: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<&'a str>,
    /// 所要時間（ms）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<&'a str>,
    pub message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<&'a Value>,
}

/// [`Logger::child`] で束ねるログコンテキスト（設計 §11）
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogContext {
    pub task_id: Option<String>,
    pub correlation_id: Option<String>,
}

impl LogContext {
    pub fn task(task_id: impl Into<String>, correlation_id: Option<String>) -> Self {
        LogContext {
            task_id: Some(task_id.into()),
            correlation_id,
        }
    }
}

/// 各ログ呼び出しで付与できる §11 フィールド
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogFields {
    /// 所要時間（ms）
    pub duration: Option<u64>,
    pub status: Option<String>,
}

impl LogFields {
    pub fn new(status: impl Into<String>, duration_ms: u64) -> Self {
        LogFields {
            duration: Some(duration_ms),
            status: Some(status.into()),
        }
    }

    pub fn status(status: impl Into<String>) -> Self {
        LogFields {
            duration: None,
            status: Some(status.into()),
        }
    }
}

/// テスト用: stderr の代わりにログ行を貯める先（`Logger::capturing` で作る。crate 内テスト専用）
#[doc(hidden)]
pub type LogSink = Arc<Mutex<Vec<String>>>;

#[derive(Debug, Clone)]
pub struct Logger {
    agent_id: Option<String>,
    level: LogLevel,
    context: LogContext,
    sink: Option<LogSink>,
}

impl Logger {
    pub fn new(agent_id: impl Into<String>) -> Self {
        Logger {
            agent_id: Some(agent_id.into()),
            level: LogLevel::from_env().unwrap_or(LogLevel::Info),
            context: LogContext::default(),
            sink: None,
        }
    }

    pub fn with_level(agent_id: Option<String>, level: LogLevel) -> Self {
        Logger {
            agent_id,
            level,
            context: LogContext::default(),
            sink: None,
        }
    }

    /// テスト用: stderr に書かず、整形済みのログ行を返り値の sink に貯めるロガー。
    /// `SUBAGENT_LOG_LEVEL` は見ない。crate 内の単体テスト専用（公開 API ではない）。
    #[cfg(test)]
    pub(crate) fn capturing(agent_id: Option<String>, level: LogLevel) -> (Self, LogSink) {
        let sink = LogSink::default();
        let logger = Logger {
            agent_id,
            level,
            context: LogContext::default(),
            sink: Some(sink.clone()),
        };
        (logger, sink)
    }

    /// task_id / correlation_id を固定した子ロガーを返す（親は変わらない）。
    ///
    /// 子の値が `None` のときは親の値を引き継ぐ。TS の `{ ...this.context, ...context }` は
    /// `undefined` の値でも親を上書きする（例: `child({ correlation_id: undefined })` で消える）が、
    /// Rust 版は意図して引き継ぐ側に倒している。値を消す手段は無い。
    pub fn child(&self, context: LogContext) -> Logger {
        Logger {
            context: LogContext {
                task_id: context.task_id.or_else(|| self.context.task_id.clone()),
                correlation_id: context
                    .correlation_id
                    .or_else(|| self.context.correlation_id.clone()),
            },
            ..self.clone()
        }
    }

    pub fn context(&self) -> &LogContext {
        &self.context
    }

    fn render(
        &self,
        level: LogLevel,
        message: &str,
        data: Option<&Value>,
        fields: &LogFields,
    ) -> Option<String> {
        let entry = LogEntry {
            timestamp: crate::types::now_iso(),
            level: level.as_str(),
            agent_id: self.agent_id.as_deref(),
            task_id: self.context.task_id.as_deref(),
            correlation_id: self.context.correlation_id.as_deref(),
            duration: fields.duration,
            status: fields.status.as_deref(),
            message,
            data,
        };
        serde_json::to_string_pretty(&entry).ok()
    }

    fn log(&self, level: LogLevel, message: &str, data: Option<&Value>, fields: &LogFields) {
        if level < self.level {
            return;
        }
        if let Some(sink) = &self.sink {
            if let Some(s) = self.render(level, message, data, fields) {
                sink.lock().unwrap_or_else(|e| e.into_inner()).push(s);
            }
            return;
        }
        if LogLevel::logging_disabled() {
            return;
        }
        if let Some(s) = self.render(level, message, data, fields) {
            let mut err = std::io::stderr().lock();
            let _ = writeln!(err, "{s}");
        }
    }

    pub fn debug(&self, message: &str, data: Option<&Value>) {
        self.log(LogLevel::Debug, message, data, &LogFields::default());
    }

    pub fn info(&self, message: &str, data: Option<&Value>) {
        self.log(LogLevel::Info, message, data, &LogFields::default());
    }

    pub fn warn(&self, message: &str, data: Option<&Value>) {
        self.log(LogLevel::Warn, message, data, &LogFields::default());
    }

    pub fn error(&self, message: &str, data: Option<&Value>) {
        self.log(LogLevel::Error, message, data, &LogFields::default());
    }

    /// duration / status 付きの debug ログ
    pub fn debug_with(&self, message: &str, data: Option<&Value>, fields: &LogFields) {
        self.log(LogLevel::Debug, message, data, fields);
    }

    /// duration / status 付きの info ログ
    pub fn info_with(&self, message: &str, data: Option<&Value>, fields: &LogFields) {
        self.log(LogLevel::Info, message, data, fields);
    }

    /// duration / status 付きの warn ログ
    pub fn warn_with(&self, message: &str, data: Option<&Value>, fields: &LogFields) {
        self.log(LogLevel::Warn, message, data, fields);
    }

    /// duration / status 付きの error ログ
    pub fn error_with(&self, message: &str, data: Option<&Value>, fields: &LogFields) {
        self.log(LogLevel::Error, message, data, fields);
    }

    pub fn set_level(&mut self, level: LogLevel) {
        self.level = level;
    }

    pub fn level(&self) -> LogLevel {
        self.level
    }
}

#[cfg(test)]
mod tests {
    //! TS 版 `src/utils/logger.test.ts`（#44）の移植
    use super::*;
    use serde_json::json;

    fn entries(sink: &LogSink) -> Vec<Value> {
        sink.lock()
            .unwrap()
            .iter()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect()
    }

    #[test]
    fn child_entries_carry_agent_task_correlation_duration_status() {
        let (logger, sink) = Logger::capturing(Some("agent-1".into()), LogLevel::Info);
        logger
            .child(LogContext::task("t1", Some("c1".into())))
            .info_with("done", None, &LogFields::new("success", 12));
        let e = &entries(&sink)[0];
        assert_eq!(e["agent_id"], "agent-1");
        assert_eq!(e["task_id"], "t1");
        assert_eq!(e["correlation_id"], "c1");
        assert_eq!(e["status"], "success");
        assert_eq!(e["duration"], 12);
        assert_eq!(e["message"], "done");
    }

    #[test]
    fn parent_does_not_get_child_task_id() {
        let (parent, sink) = Logger::capturing(Some("a".into()), LogLevel::Info);
        let _child = parent.child(LogContext::task("t1", None));
        parent.info("x", None);
        let e = &entries(&sink)[0];
        assert!(e.get("task_id").is_none(), "{e}");
        assert_eq!(parent.context(), &LogContext::default());
    }

    #[test]
    fn child_of_child_merges_context() {
        let (logger, sink) = Logger::capturing(None, LogLevel::Info);
        logger
            .child(LogContext {
                task_id: None,
                correlation_id: Some("c1".into()),
            })
            .child(LogContext::task("t2", None))
            .warn("w", Some(&json!({ "k": 1 })));
        let e = &entries(&sink)[0];
        assert_eq!(e["task_id"], "t2");
        assert_eq!(e["correlation_id"], "c1");
        assert_eq!(e["data"]["k"], 1);
        assert!(e.get("agent_id").is_none());
    }

    #[test]
    fn plain_calls_omit_structured_fields_and_respect_level() {
        let (logger, sink) = Logger::capturing(Some("a".into()), LogLevel::Info);
        logger.debug("hidden", None);
        logger.info("shown", None);
        let es = entries(&sink);
        assert_eq!(es.len(), 1);
        for k in ["task_id", "correlation_id", "duration", "status", "data"] {
            assert!(es[0].get(k).is_none(), "{k}");
        }
        // child はレベルを引き継ぐ
        logger
            .child(LogContext::task("t", None))
            .debug("hidden", None);
        assert_eq!(entries(&sink).len(), 1);
    }

    #[test]
    fn entry_key_order_matches_ts() {
        let (logger, sink) = Logger::capturing(Some("a".into()), LogLevel::Info);
        logger
            .child(LogContext::task("t", Some("c".into())))
            .error_with("m", Some(&json!(1)), &LogFields::new("failed", 3));
        let e = &entries(&sink)[0];
        let keys: Vec<&str> = e.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "timestamp",
                "level",
                "agent_id",
                "task_id",
                "correlation_id",
                "duration",
                "status",
                "message",
                "data"
            ]
        );
    }
}
