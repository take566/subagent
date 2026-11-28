//! Claude Messages API の土台（raw HTTP）
//!
//! - [`types`]: request / response の型
//! - [`client`]: [`LlmClient`] trait と、鍵が無い時の [`UnconfiguredLlm`]
//! - [`config`]: 鍵・上限の設定と 1 タスクの token 予算
//! - [`anthropic`]: `POST /v1/messages` を呼ぶ [`AnthropicClient`]
//! - [`fake`]: テスト用の [`ScriptedLlm`]（`ReplayLlm` は feature `replay` の中だけ）
//!
//! 既定（[`default_client`]、エージェントの `new()`）は常に [`UnconfiguredLlm`] で通信しない。
//! env の鍵を読むのは bin が呼ぶ [`anthropic::shared_from_env`] だけ。

pub mod anthropic;
pub mod client;
pub mod config;
pub mod fake;
pub mod types;

use std::sync::Arc;

pub use anthropic::{AnthropicClient, shared_from_env};
pub use client::{BudgetScope, LlmClient, LlmError, UnconfiguredLlm};
pub use config::{ApiKey, LlmConfig, TokenBudget};
#[cfg(feature = "replay")]
pub use fake::ReplayLlm;
pub use fake::ScriptedLlm;
pub use types::{
    CacheControl, CacheType, DEFAULT_MODEL, Effort, Message, MessageRequest, MessageResponse,
    OutputConfig, Role, StopReason, TEXT_EDITOR_NAME, TEXT_EDITOR_TYPE, ThinkingConfig,
    ThinkingDisplay, ToolDef, ToolUse, Usage, WEB_FETCH_TYPE, WEB_SEARCH_TYPE,
};

/// env を読まない既定の client（常に [`UnconfiguredLlm`]）
pub fn default_client() -> Arc<dyn LlmClient> {
    Arc::new(UnconfiguredLlm)
}
