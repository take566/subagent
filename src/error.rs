//! エラー型

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Tool {tool} not available for {agent}")]
    ToolNotAvailable { tool: String, agent: String },

    #[error("Tool {tool} is not allowed for {agent}")]
    ToolNotAllowed { tool: String, agent: String },

    #[error("No suitable subagent found for action: {0}")]
    NoSubagent(String),

    #[error("No subagent registered with capability \"{capability}\" for action: {action}")]
    NoCapableSubagent { capability: String, action: String },

    #[error("Invalid parameter: {0}")]
    InvalidParameter(String),

    #[error("{0}")]
    Io(#[from] std::io::Error),

    #[error("{source}: '{path}'")]
    IoPath {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("{0}")]
    Regex(#[from] regex::Error),

    #[error("{0}")]
    Json(#[from] serde_json::Error),

    /// sandbox（Workspace / run_command）が拒否した操作
    #[error("Sandbox: {0}")]
    Sandbox(String),

    #[error("{0}")]
    Llm(#[from] crate::llm::LlmError),

    /// typed tool が途中で止まった。`message` は短い要約だけ（ログにも出る。モデルの本文や
    /// エラーの中身は入れない）。`partial` は途中の結果で、MCP の応答では isError の
    /// `structuredContent` になる（ログには出さない）
    #[error("{message}")]
    Incomplete {
        message: String,
        partial: Box<serde_json::Value>,
    },

    #[error("{0}")]
    Other(String),
}

impl Error {
    /// JS の `error.name` に相当する名前
    pub fn name(&self) -> &'static str {
        match self {
            Error::ToolNotAvailable { .. } => "ToolNotAvailable",
            Error::ToolNotAllowed { .. } => "ToolNotAllowed",
            Error::NoSubagent(_) | Error::NoCapableSubagent { .. } => "NoSubagent",
            Error::InvalidParameter(_) => "InvalidParameter",
            Error::Io(_) | Error::IoPath { .. } => "IoError",
            Error::Regex(_) => "RegexError",
            Error::Json(_) => "JsonError",
            Error::Sandbox(_) => "SandboxError",
            Error::Llm(_) => "LlmError",
            Error::Incomplete { .. } => "Incomplete",
            Error::Other(_) => "Error",
        }
    }

    pub fn other(msg: impl Into<String>) -> Self {
        Error::Other(msg.into())
    }

    /// パス付きの IO エラーを作る
    pub fn io_at(path: impl AsRef<std::path::Path>, source: std::io::Error) -> Self {
        Error::IoPath {
            path: path.as_ref().to_string_lossy().into_owned(),
            source,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
