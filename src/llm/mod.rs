//! Claude Messages API の土台（raw HTTP）
//!
//! - [`types`]: request / response の型
//! - [`client`]: [`LlmClient`] trait と、鍵が無い時の [`UnconfiguredLlm`]
//! - [`fake`]: テスト用の [`ScriptedLlm`]（`ReplayLlm` は feature `replay` の中だけ）

pub mod client;
pub mod fake;
pub mod types;

pub use client::{LlmClient, LlmError, UnconfiguredLlm};
#[cfg(feature = "replay")]
pub use fake::ReplayLlm;
pub use fake::ScriptedLlm;
pub use types::{
    CacheControl, CacheType, DEFAULT_MODEL, Effort, Message, MessageRequest, MessageResponse,
    OutputConfig, Role, StopReason, TEXT_EDITOR_NAME, TEXT_EDITOR_TYPE, ThinkingConfig,
    ThinkingDisplay, ToolDef, ToolUse, Usage, WEB_FETCH_TYPE, WEB_SEARCH_TYPE,
};
