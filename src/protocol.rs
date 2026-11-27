//! A2A (Agent-to-Agent) 通信プロトコル実装

use crate::types::{
    now_iso, A2AMessage, MessageMetadata, MessageType, Priority, TaskMetrics, TaskRequestPayload,
    TaskResultPayload, TaskStatus,
};
use serde_json::{json, Value};
use uuid::Uuid;

pub struct A2AProtocol;

fn base_message(from: &str, to: &str, message_type: MessageType) -> A2AMessage {
    A2AMessage {
        message_id: Uuid::new_v4().to_string(),
        in_reply_to: None,
        timestamp: now_iso(),
        from: from.into(),
        to: to.into(),
        message_type: Some(message_type),
        status: None,
        payload: None,
        metadata: None,
        metrics: None,
    }
}

impl A2AProtocol {
    /// タスクリクエストメッセージを作成（priority の既定は medium）
    pub fn create_task_request(
        from: &str,
        to: &str,
        payload: &TaskRequestPayload,
        priority: Option<Priority>,
        correlation_id: Option<String>,
    ) -> A2AMessage {
        A2AMessage {
            payload: Some(serde_json::to_value(payload).unwrap_or(Value::Null)),
            metadata: Some(MessageMetadata {
                priority: Some(priority.unwrap_or(Priority::Medium)),
                correlation_id,
                extra: Default::default(),
            }),
            ..base_message(from, to, MessageType::TaskRequest)
        }
    }

    /// タスク結果レスポンスを作成
    pub fn create_task_result(
        from: &str,
        to: &str,
        in_reply_to: &str,
        payload: &TaskResultPayload,
        status: TaskStatus,
        metrics: Option<TaskMetrics>,
    ) -> A2AMessage {
        A2AMessage {
            in_reply_to: Some(in_reply_to.into()),
            status: Some(status),
            payload: Some(serde_json::to_value(payload).unwrap_or(Value::Null)),
            metrics,
            ..base_message(from, to, MessageType::TaskResult)
        }
    }

    /// プログレスメッセージを作成
    pub fn create_progress_message(
        from: &str,
        to: &str,
        task_id: &str,
        progress: f64,
        message: &str,
    ) -> A2AMessage {
        A2AMessage {
            payload: Some(json!({
                "task_id": task_id,
                "progress": crate::types::js_number(progress),
                "message": message,
            })),
            ..base_message(from, to, MessageType::Progress)
        }
    }

    /// エラーメッセージを作成
    pub fn create_error_message(
        from: &str,
        to: &str,
        in_reply_to: &str,
        code: &str,
        message: &str,
        details: Option<Value>,
    ) -> A2AMessage {
        let mut error = json!({ "code": code, "message": message });
        if let Some(d) = details {
            error["details"] = d;
        }
        A2AMessage {
            in_reply_to: Some(in_reply_to.into()),
            status: Some(TaskStatus::Failed),
            payload: Some(json!({ "error": error })),
            ..base_message(from, to, MessageType::Error)
        }
    }

    /// ハートビートメッセージを作成
    pub fn create_heartbeat(from: &str, to: &str) -> A2AMessage {
        A2AMessage {
            payload: Some(json!({ "status": "alive" })),
            ..base_message(from, to, MessageType::Heartbeat)
        }
    }

    /// メッセージを検証
    pub fn validate_message(message: &A2AMessage) -> Result<(), String> {
        if message.message_id.is_empty() {
            return Err("Missing message_id".into());
        }
        if message.timestamp.is_empty() {
            return Err("Missing timestamp".into());
        }
        if message.from.is_empty() {
            return Err("Missing from".into());
        }
        if message.to.is_empty() {
            return Err("Missing to".into());
        }
        let Some(t) = message.message_type else {
            return Err("Missing type".into());
        };
        if matches!(t, MessageType::TaskResult | MessageType::Error)
            && message.in_reply_to.as_deref().unwrap_or("").is_empty()
        {
            return Err("Missing in_reply_to for response message".into());
        }
        Ok(())
    }

    /// メッセージをシリアライズ
    pub fn serialize(message: &A2AMessage) -> String {
        serde_json::to_string_pretty(message).unwrap_or_default()
    }

    /// メッセージをデシリアライズ
    pub fn deserialize(data: &str) -> serde_json::Result<A2AMessage> {
        serde_json::from_str(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_validate() {
        let payload = TaskRequestPayload {
            task_id: "t1".into(),
            action: "research".into(),
            parameters: Default::default(),
            context: None,
        };
        let m = A2AProtocol::create_task_request("a", "b", &payload, None, None);
        assert_eq!(m.metadata.as_ref().unwrap().priority, Some(Priority::Medium));
        assert!(A2AProtocol::validate_message(&m).is_ok());
        let s = A2AProtocol::serialize(&m);
        assert!(s.contains("\"type\": \"task_request\""));
        assert_eq!(A2AProtocol::deserialize(&s).unwrap(), m);
    }

    #[test]
    fn validation_errors() {
        let mut m = A2AProtocol::create_heartbeat("a", "b");
        assert!(A2AProtocol::validate_message(&m).is_ok());
        m.from.clear();
        assert_eq!(A2AProtocol::validate_message(&m).unwrap_err(), "Missing from");

        let mut r = A2AProtocol::create_error_message("a", "b", "x", "C", "m", None);
        assert!(A2AProtocol::validate_message(&r).is_ok());
        r.in_reply_to = None;
        assert_eq!(
            A2AProtocol::validate_message(&r).unwrap_err(),
            "Missing in_reply_to for response message"
        );

        let raw = A2AProtocol::deserialize(r#"{"message_id":"1","timestamp":"t","from":"a","to":"b"}"#).unwrap();
        assert_eq!(A2AProtocol::validate_message(&raw).unwrap_err(), "Missing type");
    }
}
