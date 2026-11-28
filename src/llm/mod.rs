//! Claude Messages API の土台（raw HTTP）
//!
//! - [`types`]: request / response の型
//! - [`client`]: [`LlmClient`] trait と、鍵が無い時の [`UnconfiguredLlm`]
//! - [`config`]: 鍵・上限の設定と 1 タスクの token 予算
//! - [`anthropic`]: `POST /v1/messages` を呼ぶ [`AnthropicClient`]
//! - [`runner`]: 全エージェント共通の往復ループ [`ConversationRunner`]
//! - [`settings`]: エージェントごとの設定 [`AgentLlmSettings`] と bin の backend 選択 [`client_from_env`]
//! - [`fake`]: テスト用の [`ScriptedLlm`]（`ReplayLlm` は feature `replay` の中だけ）
//!
//! 既定（[`default_client`]、エージェントの `new()`）は常に [`UnconfiguredLlm`] で通信しない。
//! env の鍵を読むのは bin が呼ぶ [`client_from_env`]（→ [`anthropic::shared_from_env`]）だけ。

pub mod anthropic;
pub mod client;
pub mod config;
pub mod fake;
pub mod runner;
pub mod settings;
pub mod types;

use std::sync::Arc;

pub use anthropic::{AnthropicClient, shared_from_env};
pub use client::{BudgetScope, LlmClient, LlmError, UnconfiguredLlm};
pub use config::{ApiKey, LlmConfig, TokenBudget};
#[cfg(feature = "replay")]
pub use fake::ReplayLlm;
pub use fake::ScriptedLlm;
pub use runner::{
    ConversationRunner, DEFAULT_MAX_TOKENS_CEILING, RunObserver, RunOutcome, RunStop, RunnerLimits,
    ToolHandler,
};
pub use settings::{
    AgentKind, AgentLlm, AgentLlmSettings, LLM_BACKEND_ENV, LLM_REPLAY_FILE_ENV, LlmBackend,
    client_from_env, select_backend,
};
pub use types::{
    CacheControl, CacheType, DEFAULT_MODEL, Effort, Message, MessageRequest, MessageResponse,
    OutputConfig, Role, StopReason, TEXT_EDITOR_NAME, TEXT_EDITOR_TYPE, ThinkingConfig,
    ThinkingDisplay, ToolDef, ToolUse, Usage, WEB_FETCH_TYPE, WEB_SEARCH_TYPE,
};

/// env を読まない既定の client（常に [`UnconfiguredLlm`]）
pub fn default_client() -> Arc<dyn LlmClient> {
    Arc::new(UnconfiguredLlm)
}
