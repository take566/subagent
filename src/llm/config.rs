//! LLM の設定（鍵・接続・上限）と 1 タスクの token 予算

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use super::client::{BudgetScope, LlmError};

/// 優先して読む鍵の env
pub const API_KEY_ENV: &str = "SUBAGENT_ANTHROPIC_API_KEY";
/// 上が無い時に読む鍵の env
pub const FALLBACK_API_KEY_ENV: &str = "ANTHROPIC_API_KEY";
/// process 単位の 1 日の token 上限の env
pub const MAX_TOKENS_PER_DAY_ENV: &str = "SUBAGENT_MAX_TOKENS_PER_DAY";
/// 1 日の token 上限の既定値
pub const DEFAULT_MAX_TOKENS_PER_DAY: u64 = 2_000_000;
/// API の base URL
pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
/// `anthropic-version` header
pub const ANTHROPIC_VERSION: &str = "2026-10-21";
/// 同時に送る request の上限（process で共有）
pub const DEFAULT_MAX_CONCURRENCY: usize = 4;

/// API 鍵。Debug / Display に値を出さない
#[derive(Clone)]
pub struct ApiKey(String);

impl ApiKey {
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    /// header に入れる時だけ使う
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ApiKey(<redacted>)")
    }
}

/// [`super::AnthropicClient`] の設定
#[derive(Debug, Clone)]
pub struct LlmConfig {
    pub api_key: ApiKey,
    pub base_url: String,
    pub max_concurrency: usize,
    /// 最初の送信のあとに retry する回数
    pub max_retries: u32,
    /// 指数 backoff の基準
    pub retry_base_delay: Duration,
    /// これより長い `retry-after` は待たずに失敗する
    pub max_retry_after: Duration,
    pub connect_timeout: Duration,
    /// 1 request 全体の timeout（送信後に切れたら retry しない）
    pub request_timeout: Duration,
    /// process 単位の 1 日の token 上限
    pub max_tokens_per_day: u64,
}

impl LlmConfig {
    /// 既定値と鍵で作る（env は読まない）
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: ApiKey::new(api_key),
            base_url: DEFAULT_BASE_URL.to_string(),
            max_concurrency: DEFAULT_MAX_CONCURRENCY,
            max_retries: 2,
            retry_base_delay: Duration::from_millis(500),
            max_retry_after: Duration::from_secs(60),
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(600),
            max_tokens_per_day: DEFAULT_MAX_TOKENS_PER_DAY,
        }
    }

    /// process の env から読む。鍵が無ければ `Ok(None)`
    pub fn from_env() -> Result<Option<Self>, LlmError> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// 任意の lookup から読む（テスト用に env を差し替えられる）
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Option<Self>, LlmError> {
        let non_empty = |k: &str| lookup(k).filter(|v| !v.trim().is_empty());
        let Some(key) = non_empty(API_KEY_ENV).or_else(|| non_empty(FALLBACK_API_KEY_ENV)) else {
            return Ok(None);
        };
        let mut config = Self::new(key.trim());
        if let Some(raw) = non_empty(MAX_TOKENS_PER_DAY_ENV) {
            config.max_tokens_per_day = raw.trim().parse().map_err(|_| {
                LlmError::InvalidConfig(format!(
                    "{MAX_TOKENS_PER_DAY_ENV} must be a non-negative integer"
                ))
            })?;
        }
        Ok(Some(config))
    }
}

/// 1 タスクの token 予算（応答の usage を足していく）
#[derive(Debug)]
pub struct TokenBudget {
    limit: u64,
    used: AtomicU64,
}

impl TokenBudget {
    pub fn new(limit: u64) -> Self {
        Self {
            limit,
            used: AtomicU64::new(0),
        }
    }

    /// research の既定（300k）
    pub fn research() -> Self {
        Self::new(300_000)
    }

    /// codegen の既定（1M）
    pub fn codegen() -> Self {
        Self::new(1_000_000)
    }

    /// review の既定（100k）
    pub fn review() -> Self {
        Self::new(100_000)
    }

    /// document の既定（100k）
    pub fn document() -> Self {
        Self::new(100_000)
    }

    pub fn limit(&self) -> u64 {
        self.limit
    }

    pub fn used(&self) -> u64 {
        self.used.load(Ordering::Relaxed)
    }

    pub fn remaining(&self) -> u64 {
        self.limit.saturating_sub(self.used())
    }

    /// 使った token を足す
    pub fn charge(&self, tokens: u64) {
        self.used.fetch_add(tokens, Ordering::Relaxed);
    }

    /// 使い切っていれば [`LlmError::BudgetExceeded`]
    pub fn check(&self) -> Result<(), LlmError> {
        let used = self.used();
        if used >= self.limit {
            Err(LlmError::BudgetExceeded {
                scope: BudgetScope::Task,
                used,
                limit: self.limit,
            })
        } else {
            Ok(())
        }
    }
}
