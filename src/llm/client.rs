//! LLM client の trait と、鍵が無い時の [`UnconfiguredLlm`]

use async_trait::async_trait;
use thiserror::Error;
use tokio::time::Instant;

use super::types::{MessageRequest, MessageResponse};

/// LLM 呼び出しのエラー。API 鍵の値は含めない
#[derive(Debug, Error)]
pub enum LlmError {
    /// API 鍵が設定されていない（[`UnconfiguredLlm`]）
    #[error("LLM is not configured (set SUBAGENT_ANTHROPIC_API_KEY)")]
    NotConfigured,

    /// API が非 2xx を返した
    #[error("Claude API error {status} {error_type}: {message}")]
    Api {
        status: u16,
        error_type: String,
        message: String,
        request_id: Option<String>,
    },

    /// 応答を受け取る前の通信エラー
    #[error("Claude API transport error after {attempts} attempt(s): {message}")]
    Transport { message: String, attempts: u32 },

    /// 送信後の timeout（二重課金を避けるため retry しない）。
    /// [`super::AnthropicClient`] は代わりに [`LlmError::PossiblyBilled`] を返す
    #[error("Claude API request timed out after it was sent")]
    Timeout,

    /// 応答 body を解釈できない
    #[error("Claude API response could not be decoded: {0}")]
    Decode(String),

    /// 送った後に失敗した（送信後の timeout・body の読み出し失敗・2xx の decode 失敗）。
    /// 課金された可能性があるので retry せず、見積もり（予約した token 数）を使用量に数える
    #[error(
        "Claude API request may have been billed but failed ({reason}); counted {estimated_tokens} tokens"
    )]
    PossiblyBilled {
        reason: String,
        estimated_tokens: u64,
    },

    /// deadline を過ぎていたので送らなかった（送信中に過ぎた時は [`LlmError::PossiblyBilled`]）
    #[error("deadline exceeded before the Claude API request was sent")]
    DeadlineExceeded,

    /// token 予算を使い切った（送信しない）
    #[error("{scope} token budget exceeded: used {used} of {limit}")]
    BudgetExceeded {
        scope: BudgetScope,
        used: u64,
        limit: u64,
    },

    /// 設定値が不正
    #[error("invalid LLM configuration: {0}")]
    InvalidConfig(String),

    #[error("{0}")]
    Other(String),
}

impl LlmError {
    /// 課金された可能性があり、予算に数えるべき token 数（無ければ 0）
    pub fn billed_estimate(&self) -> u64 {
        match self {
            LlmError::PossiblyBilled {
                estimated_tokens, ..
            } => *estimated_tokens,
            _ => 0,
        }
    }
}

/// どの予算を使い切ったか
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetScope {
    /// 1 タスクの予算
    Task,
    /// process 単位の 1 日上限
    Daily,
}

impl std::fmt::Display for BudgetScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            BudgetScope::Task => "task",
            BudgetScope::Daily => "daily",
        })
    }
}

/// Messages API を呼ぶ client
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// `POST /v1/messages` を 1 回（client 内の retry を含む）呼ぶ
    async fn create_message(&self, req: &MessageRequest) -> Result<MessageResponse, LlmError>;

    /// deadline 付きで呼ぶ。過ぎていれば送らずに [`LlmError::DeadlineExceeded`]。
    ///
    /// 既定の実装は future を deadline で打ち切るだけ。[`super::AnthropicClient`] は
    /// request の timeout を deadline までに縮め、送信後に切れた分を課金ありとして数える。
    async fn create_message_until(
        &self,
        req: &MessageRequest,
        deadline: Option<Instant>,
    ) -> Result<MessageResponse, LlmError> {
        let Some(deadline) = deadline else {
            return self.create_message(req).await;
        };
        if Instant::now() >= deadline {
            return Err(LlmError::DeadlineExceeded);
        }
        tokio::time::timeout_at(deadline, self.create_message(req))
            .await
            .unwrap_or(Err(LlmError::DeadlineExceeded))
    }

    /// 実際に呼べる client か（[`UnconfiguredLlm`] は false）
    fn is_configured(&self) -> bool {
        true
    }
}

/// 鍵が無い時の client。常に [`LlmError::NotConfigured`] を返し、通信しない
#[derive(Debug, Clone, Copy, Default)]
pub struct UnconfiguredLlm;

#[async_trait]
impl LlmClient for UnconfiguredLlm {
    async fn create_message(&self, _req: &MessageRequest) -> Result<MessageResponse, LlmError> {
        Err(LlmError::NotConfigured)
    }

    fn is_configured(&self) -> bool {
        false
    }
}
