//! LLM client の trait と、鍵が無い時の [`UnconfiguredLlm`]

use async_trait::async_trait;
use thiserror::Error;

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

    /// 送信後の timeout（二重課金を避けるため retry しない）
    #[error("Claude API request timed out after it was sent")]
    Timeout,

    /// 応答 body を解釈できない
    #[error("Claude API response could not be decoded: {0}")]
    Decode(String),

    #[error("{0}")]
    Other(String),
}

/// Messages API を呼ぶ client
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// `POST /v1/messages` を 1 回（client 内の retry を含む）呼ぶ
    async fn create_message(&self, req: &MessageRequest) -> Result<MessageResponse, LlmError>;

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
