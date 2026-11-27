//! ロガー実装
//!
//! 構造化ログ（JSON）を **stderr** に出力する。MCP の stdio トランスポートは
//! stdout を JSON-RPC 専用に使うため、ログを stdout に出してはいけない。

use serde::Serialize;
use serde_json::Value;
use std::io::Write;

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
    pub message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<&'a Value>,
}

#[derive(Debug, Clone)]
pub struct Logger {
    agent_id: Option<String>,
    level: LogLevel,
}

impl Logger {
    pub fn new(agent_id: impl Into<String>) -> Self {
        Logger {
            agent_id: Some(agent_id.into()),
            level: LogLevel::from_env().unwrap_or(LogLevel::Info),
        }
    }

    pub fn with_level(agent_id: Option<String>, level: LogLevel) -> Self {
        Logger { agent_id, level }
    }

    fn log(&self, level: LogLevel, message: &str, data: Option<&Value>) {
        if level < self.level || LogLevel::logging_disabled() {
            return;
        }
        let entry = LogEntry {
            timestamp: crate::types::now_iso(),
            level: level.as_str(),
            agent_id: self.agent_id.as_deref(),
            task_id: None,
            correlation_id: None,
            message,
            data,
        };
        if let Ok(s) = serde_json::to_string_pretty(&entry) {
            let mut err = std::io::stderr().lock();
            let _ = writeln!(err, "{s}");
        }
    }

    pub fn debug(&self, message: &str, data: Option<&Value>) {
        self.log(LogLevel::Debug, message, data);
    }

    pub fn info(&self, message: &str, data: Option<&Value>) {
        self.log(LogLevel::Info, message, data);
    }

    pub fn warn(&self, message: &str, data: Option<&Value>) {
        self.log(LogLevel::Warn, message, data);
    }

    pub fn error(&self, message: &str, data: Option<&Value>) {
        self.log(LogLevel::Error, message, data);
    }

    pub fn set_level(&mut self, level: LogLevel) {
        self.level = level;
    }

    pub fn level(&self) -> LogLevel {
        self.level
    }
}
