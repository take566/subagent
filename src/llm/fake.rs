//! テスト用の LLM client
//!
//! - [`ScriptedLlm`]: 決めた順に応答を返し、受けた request を記録する。
//! - `ReplayLlm`: 記録済みの応答を file から読んで返す。cargo feature `replay` の中だけにあり、
//!   本番 binary では使えない。

use std::collections::VecDeque;
use std::sync::Mutex;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::client::{LlmClient, LlmError};
use super::types::{MessageRequest, MessageResponse, StopReason, Usage};

/// 決めた順に応答を返す client
#[derive(Debug, Default)]
pub struct ScriptedLlm {
    script: Mutex<VecDeque<Result<MessageResponse, LlmError>>>,
    requests: Mutex<Vec<Value>>,
}

impl ScriptedLlm {
    pub fn new(script: Vec<Result<MessageResponse, LlmError>>) -> Self {
        Self {
            script: Mutex::new(script.into()),
            requests: Mutex::new(Vec::new()),
        }
    }

    /// 受けた request（JSON にしたもの）を順に返す
    pub fn requests(&self) -> Vec<Value> {
        self.requests.lock().unwrap().clone()
    }

    /// 残りの応答数
    pub fn remaining(&self) -> usize {
        self.script.lock().unwrap().len()
    }

    /// 任意の content と stop_reason の応答
    pub fn response(stop_reason: StopReason, content: Vec<Value>) -> MessageResponse {
        MessageResponse {
            id: "msg_scripted".into(),
            model: super::types::DEFAULT_MODEL.into(),
            content,
            stop_reason,
            stop_sequence: None,
            usage: Usage {
                input_tokens: 10,
                output_tokens: 10,
                ..Usage::default()
            },
        }
    }

    /// text 1 個で end_turn の応答
    pub fn text_response(text: &str) -> MessageResponse {
        Self::response(
            StopReason::EndTurn,
            vec![json!({"type": "text", "text": text})],
        )
    }
}

#[async_trait]
impl LlmClient for ScriptedLlm {
    async fn create_message(&self, req: &MessageRequest) -> Result<MessageResponse, LlmError> {
        let recorded = serde_json::to_value(req).map_err(|e| LlmError::Other(e.to_string()))?;
        self.requests.lock().unwrap().push(recorded);
        self.script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Err(LlmError::Other("ScriptedLlm: script exhausted".into())))
    }
}

#[cfg(feature = "replay")]
pub use replay::ReplayLlm;

#[cfg(feature = "replay")]
mod replay {
    use super::*;
    use std::path::Path;

    /// 記録済みの応答（JSON の配列）を順に返す client。feature `replay` の中だけ
    #[derive(Debug)]
    pub struct ReplayLlm {
        inner: ScriptedLlm,
    }

    impl ReplayLlm {
        /// 応答 object の JSON 配列を読む
        pub fn from_json(text: &str) -> Result<Self, LlmError> {
            let responses: Vec<MessageResponse> =
                serde_json::from_str(text).map_err(|e| LlmError::Decode(e.to_string()))?;
            Ok(Self {
                inner: ScriptedLlm::new(responses.into_iter().map(Ok).collect()),
            })
        }

        /// file から読む
        pub fn from_file(path: impl AsRef<Path>) -> Result<Self, LlmError> {
            let path = path.as_ref();
            let text = std::fs::read_to_string(path)
                .map_err(|e| LlmError::Other(format!("{}: {e}", path.display())))?;
            Self::from_json(&text)
        }

        /// 受けた request
        pub fn requests(&self) -> Vec<Value> {
            self.inner.requests()
        }
    }

    #[async_trait]
    impl LlmClient for ReplayLlm {
        async fn create_message(&self, req: &MessageRequest) -> Result<MessageResponse, LlmError> {
            self.inner.create_message(req).await
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::llm::Message;

        #[tokio::test]
        async fn replay_serves_recorded_responses_in_order() {
            let llm = ReplayLlm::from_json(
                r#"[{"id":"a","model":"claude-opus-5-5","content":[{"type":"text","text":"hi"}],
                     "stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}]"#,
            )
            .unwrap();
            let req = MessageRequest::new(10, vec![Message::user_text("x")]);
            assert_eq!(llm.create_message(&req).await.unwrap().text(), "hi");
            assert!(llm.create_message(&req).await.is_err());
            assert_eq!(llm.requests().len(), 2);
        }

        #[test]
        fn replay_rejects_malformed_fixture() {
            assert!(matches!(
                ReplayLlm::from_json("{not json"),
                Err(LlmError::Decode(_))
            ));
        }
    }
}
