//! A2A (Agent-to-Agent) 通信プロトコル実装

use crate::types::{
    A2AMessage, MessageMetadata, MessageType, Priority, TaskMetrics, TaskRequestPayload,
    TaskResultPayload, TaskStatus, now_iso,
};
use serde_json::{Value, json};
use uuid::Uuid;

pub struct A2AProtocol;

fn is_non_empty_string(v: Option<&Value>) -> bool {
    matches!(v, Some(Value::String(s)) if !s.is_empty())
}

fn is_error_info(v: Option<&Value>) -> bool {
    v.and_then(Value::as_object).is_some_and(|o| {
        o.get("code").is_some_and(Value::is_string)
            && o.get("message").is_some_and(Value::is_string)
    })
}

/// JS の `String(v)` 相当（エラーメッセージ用）
fn js_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Object(_) => "[object Object]".into(),
        Value::Array(a) => a
            .iter()
            .map(|x| match x {
                Value::Null => String::new(),
                x => js_string(x),
            })
            .collect::<Vec<_>>()
            .join(","),
        other => other.to_string(),
    }
}

/// `Date.parse` が NaN にならない文字列か。
///
/// 有効とする書式（日付と時刻の区切りは `T` でも空白でもよい。V8 の `Date.parse` と同じ）:
/// - RFC 3339（`Z` / `±hh:mm` 付き、小数秒可）
/// - タイムゾーン無しの日時（`YYYY-MM-DDThh:mm[:ss[.f]]`）
/// - 日付のみ（`YYYY-MM-DD`）
///
/// うるう秒（`:60`）や存在しない日付・時刻は `Date.parse` と同じく無効。
///
/// 意図して TS と違う点: `Date.parse` が実装依存で受ける ISO 8601 以外の書式
/// （`Tue, 01 Jan 2024 ...`、`2024/01/01`、`Jan 1 2024`、年のみ `2024`、年月のみ `2024-01`、
/// 拡張年 `+002024-...` など）は無効とする。
fn is_valid_date(s: &str) -> bool {
    use chrono::{DateTime, NaiveDate, NaiveDateTime, Timelike};
    // 日付と時刻の間の空白を `T` にそろえる
    let normalized;
    let s = if s.len() > 10 && s.is_char_boundary(10) && s.as_bytes()[10] == b' ' {
        normalized = format!("{}T{}", &s[..10], &s[11..]);
        normalized.as_str()
    } else {
        s
    };
    // chrono はうるう秒を nanosecond >= 1_000_000_000 で表す
    let not_leap = |nanos: u32| nanos < 1_000_000_000;
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return not_leap(dt.nanosecond());
    }
    for fmt in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M"] {
        if let Ok(dt) = NaiveDateTime::parse_from_str(s, fmt) {
            return not_leap(dt.nanosecond());
        }
    }
    NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
}

/// 種別ごとのペイロード検証（TS の `validatePayload`）
fn validate_payload(message_type: MessageType, payload: Option<&Value>) -> Result<(), String> {
    let type_name = serde_json::to_value(message_type)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default();
    let Some(p) = payload.and_then(Value::as_object) else {
        return Err(format!("payload must be an object for {type_name}"));
    };
    let fail = |msg: &str| Err(format!("{type_name} payload: {msg}"));
    match message_type {
        MessageType::TaskRequest => {
            if !is_non_empty_string(p.get("task_id")) {
                return fail("task_id must be a non-empty string");
            }
            if !is_non_empty_string(p.get("action")) {
                return fail("action must be a non-empty string");
            }
            if !p.get("parameters").is_some_and(Value::is_object) {
                return fail("parameters must be an object");
            }
            if p.get("context").is_some_and(|c| !c.is_object()) {
                return fail("context must be an object");
            }
        }
        MessageType::TaskResult => {
            if !is_non_empty_string(p.get("task_id")) {
                return fail("task_id must be a non-empty string");
            }
            if p.contains_key("error") && !is_error_info(p.get("error")) {
                return fail("error must have string code and message");
            }
        }
        MessageType::Progress => {
            if !is_non_empty_string(p.get("task_id")) {
                return fail("task_id must be a non-empty string");
            }
            // serde_json の Number は常に有限（NaN / Infinity は null になる）
            if !p.get("progress").is_some_and(Value::is_number) {
                return fail("progress must be a finite number");
            }
            if !p.get("message").is_some_and(Value::is_string) {
                return fail("message must be a string");
            }
        }
        MessageType::Error => {
            if !is_error_info(p.get("error")) {
                return fail("error must have string code and message");
            }
        }
        MessageType::Heartbeat => {
            if p.get("status").and_then(Value::as_str) != Some("alive") {
                return fail("status must be 'alive'");
            }
        }
    }
    Ok(())
}

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

    /// メッセージを検証（構造体版）。JSON にしてから [`Self::validate_value`] と同じ規則で見る。
    pub fn validate_message(message: &A2AMessage) -> Result<(), String> {
        let value = serde_json::to_value(message).map_err(|e| e.to_string())?;
        Self::validate_value(&value)
    }

    /// メッセージを検証（TS の `validateMessage(unknown)`）。
    ///
    /// object であること、必須 key の型、timestamp の日付妥当性、type の値、
    /// response の in_reply_to / status、metadata の型、type ごとの payload を見る。
    /// JSON には `undefined` が無いので、TS の「`!== undefined` なら検証」は「key があれば検証」とする。
    pub fn validate_value(message: &Value) -> Result<(), String> {
        let Some(m) = message.as_object() else {
            return Err("Message must be an object".into());
        };
        for key in ["message_id", "timestamp", "from", "to"] {
            match m.get(key) {
                None | Some(Value::Null) => return Err(format!("Missing {key}")),
                Some(Value::String(s)) if s.is_empty() => return Err(format!("Missing {key}")),
                Some(Value::String(_)) => {}
                Some(_) => return Err(format!("{key} must be a non-empty string")),
            }
        }
        if !is_valid_date(m["timestamp"].as_str().unwrap_or_default()) {
            return Err("timestamp must be a valid date string".into());
        }
        let message_type = match m.get("type") {
            None | Some(Value::Null) => return Err("Missing type".into()),
            Some(Value::String(s)) if s.is_empty() => return Err("Missing type".into()),
            Some(v) => serde_json::from_value::<MessageType>(v.clone())
                .map_err(|_| format!("Unknown message type: {}", js_string(v)))?,
        };

        if matches!(message_type, MessageType::TaskResult | MessageType::Error) {
            if !is_non_empty_string(m.get("in_reply_to")) {
                return Err("Missing in_reply_to for response message".into());
            }
            if let Some(status) = m.get("status")
                && serde_json::from_value::<TaskStatus>(status.clone()).is_err()
            {
                return Err("Invalid status".into());
            }
        }

        if m.get("metadata").is_some_and(|v| !v.is_object()) {
            return Err("metadata must be an object".into());
        }

        validate_payload(message_type, m.get("payload"))
    }

    /// 型ガード: 検証を通る A2A メッセージか
    pub fn is_message(value: &Value) -> bool {
        Self::validate_value(value).is_ok()
    }

    /// メッセージをシリアライズ（不正なメッセージは `Invalid A2A message: ...` のエラー）
    pub fn serialize(message: &A2AMessage) -> Result<String, String> {
        Self::validate_message(message).map_err(|e| format!("Invalid A2A message: {e}"))?;
        serde_json::to_string_pretty(message).map_err(|e| format!("Invalid A2A message: {e}"))
    }

    /// メッセージをデシリアライズ（不正な JSON / スキーマ違反はエラー）
    pub fn deserialize(data: &str) -> Result<A2AMessage, String> {
        let parsed: Value = serde_json::from_str(data)
            .map_err(|_| "Invalid A2A message: malformed JSON".to_string())?;
        Self::validate_value(&parsed).map_err(|e| format!("Invalid A2A message: {e}"))?;
        // 検証は TS と同じ範囲。priority / metrics など構造体の型に合わない値はここで弾く
        serde_json::from_value(parsed).map_err(|e| format!("Invalid A2A message: {e}"))
    }
}

#[cfg(test)]
mod tests {
    //! TS 版 `tests/protocol/a2a.test.ts`（#38 / #42）の移植
    use super::*;

    fn req_msg() -> A2AMessage {
        let payload = TaskRequestPayload {
            task_id: "t1".into(),
            action: "run".into(),
            parameters: Default::default(),
            context: None,
        };
        A2AProtocol::create_task_request("a", "b", &payload, None, None)
    }

    fn req() -> Value {
        serde_json::to_value(req_msg()).unwrap()
    }

    fn result_msg() -> Value {
        let payload = TaskResultPayload {
            task_id: "t".into(),
            result: None,
            error: None,
        };
        serde_json::to_value(A2AProtocol::create_task_result(
            "b",
            "a",
            "m1",
            &payload,
            TaskStatus::Completed,
            None,
        ))
        .unwrap()
    }

    /// `{ ...m, [key]: v }`。v が None なら key を消す（TS の `undefined`）
    fn with(m: &Value, key: &str, v: Option<Value>) -> Value {
        let mut m = m.clone();
        let o = m.as_object_mut().unwrap();
        match v {
            Some(v) => {
                o.insert(key.into(), v);
            }
            None => {
                o.remove(key);
            }
        }
        m
    }

    fn err(v: &Value) -> String {
        A2AProtocol::validate_value(v).unwrap_err()
    }

    #[test]
    fn create_task_request_defaults_priority_and_honors_overrides() {
        assert_eq!(
            req_msg().metadata.as_ref().unwrap().priority,
            Some(Priority::Medium)
        );
        let payload = TaskRequestPayload {
            task_id: "t".into(),
            action: "x".into(),
            parameters: Default::default(),
            context: None,
        };
        let m = A2AProtocol::create_task_request(
            "a",
            "b",
            &payload,
            Some(Priority::High),
            Some("c1".into()),
        );
        assert_eq!(
            serde_json::to_value(m.metadata).unwrap(),
            json!({ "priority": "high", "correlation_id": "c1" })
        );
    }

    #[test]
    fn task_result_and_error_message_set_reply_linkage_and_status() {
        let r = result_msg();
        assert_eq!(r["in_reply_to"], "m1");
        assert_eq!(r["status"], "completed");
        let e = A2AProtocol::create_error_message("b", "a", "m1", "E", "x", None);
        assert_eq!(e.status, Some(TaskStatus::Failed));
        assert_eq!(e.message_type, Some(MessageType::Error));
    }

    #[test]
    fn generates_unique_message_ids() {
        assert_ne!(req_msg().message_id, req_msg().message_id);
    }

    #[test]
    fn accepts_valid_messages() {
        assert_eq!(A2AProtocol::validate_value(&req()), Ok(()));
        assert_eq!(A2AProtocol::validate_message(&req_msg()), Ok(()));
        assert_eq!(A2AProtocol::validate_value(&result_msg()), Ok(()));
        let p = A2AProtocol::create_progress_message("b", "a", "t", 0.1, "m");
        assert_eq!(A2AProtocol::validate_message(&p), Ok(()));
        let h = A2AProtocol::create_heartbeat("a", "b");
        assert_eq!(A2AProtocol::validate_message(&h), Ok(()));
        let e = A2AProtocol::create_error_message("b", "a", "m1", "E", "x", None);
        assert_eq!(A2AProtocol::validate_message(&e), Ok(()));
    }

    #[test]
    fn rejects_non_object_input() {
        for v in [json!(null), json!(1), json!("x"), json!([])] {
            assert_eq!(err(&v), "Message must be an object", "{v}");
        }
    }

    #[test]
    fn reports_missing_or_non_string_required_keys() {
        for key in ["message_id", "timestamp", "from", "to"] {
            assert_eq!(err(&with(&req(), key, None)), format!("Missing {key}"));
            assert_eq!(
                err(&with(&req(), key, Some(json!(null)))),
                format!("Missing {key}")
            );
            assert_eq!(
                err(&with(&req(), key, Some(json!("")))),
                format!("Missing {key}")
            );
            assert_eq!(
                err(&with(&req(), key, Some(json!(1)))),
                format!("{key} must be a non-empty string")
            );
        }
    }

    #[test]
    fn rejects_invalid_timestamp_and_missing_or_unknown_type() {
        assert_eq!(
            err(&with(&req(), "timestamp", Some(json!("nope")))),
            "timestamp must be a valid date string"
        );
        assert_eq!(err(&with(&req(), "type", None)), "Missing type");
        assert_eq!(
            err(&with(&req(), "type", Some(json!("zzz")))),
            "Unknown message type: zzz"
        );
        assert_eq!(
            err(&with(&req(), "type", Some(json!(1)))),
            "Unknown message type: 1"
        );
        // 構造体経由でも type 欠落は検出する
        let mut m = req_msg();
        m.message_type = None;
        assert_eq!(
            A2AProtocol::validate_message(&m).unwrap_err(),
            "Missing type"
        );
    }

    #[test]
    fn accepts_common_timestamp_formats() {
        for ts in [
            "2026-11-28T00:00:00.000Z",
            "2026-11-28T09:00:00+09:00",
            "2026-11-28T00:00:00",
            "2026-11-28",
        ] {
            assert_eq!(
                A2AProtocol::validate_value(&with(&req(), "timestamp", Some(json!(ts)))),
                Ok(()),
                "{ts}"
            );
        }
    }

    #[test]
    fn requires_in_reply_to_and_valid_status_on_responses() {
        let r = result_msg();
        assert_eq!(
            err(&with(&r, "in_reply_to", None)),
            "Missing in_reply_to for response message"
        );
        assert_eq!(
            err(&with(&r, "status", Some(json!("bad")))),
            "Invalid status"
        );
        // 構造体経由
        let mut e = A2AProtocol::create_error_message("a", "b", "x", "C", "m", None);
        e.in_reply_to = None;
        assert_eq!(
            A2AProtocol::validate_message(&e).unwrap_err(),
            "Missing in_reply_to for response message"
        );
    }

    #[test]
    fn rejects_non_object_metadata() {
        assert_eq!(
            err(&with(&req(), "metadata", Some(json!("x")))),
            "metadata must be an object"
        );
    }

    #[test]
    fn validates_payload_per_type() {
        let bad = |m: &Value, payload: Value| {
            A2AProtocol::validate_value(&with(m, "payload", Some(payload))).is_err()
        };
        assert!(bad(
            &req(),
            json!({ "task_id": "t", "action": "a", "parameters": {}, "context": 1 })
        ));
        assert!(bad(
            &req(),
            json!({ "task_id": "", "action": "a", "parameters": {} })
        ));
        assert!(bad(
            &req(),
            json!({ "task_id": "t", "action": 1, "parameters": {} })
        ));
        assert!(bad(
            &req(),
            json!({ "task_id": "t", "action": "a", "parameters": [] })
        ));
        assert!(bad(&req(), json!("str")));
        assert_eq!(
            err(&with(&req(), "payload", None)),
            "payload must be an object for task_request"
        );
        let res = result_msg();
        assert!(bad(&res, json!({ "task_id": "t", "error": { "code": 1 } })));
        assert!(!bad(
            &res,
            json!({ "task_id": "t", "error": { "code": "E", "message": "m" } })
        ));
        let p = serde_json::to_value(A2AProtocol::create_progress_message(
            "b", "a", "t", 0.1, "m",
        ))
        .unwrap();
        assert!(bad(
            &p,
            json!({ "task_id": "t", "progress": "1", "message": "m" })
        ));
        assert!(bad(
            &p,
            json!({ "task_id": "t", "progress": 1, "message": 2 })
        ));
        let e = serde_json::to_value(A2AProtocol::create_error_message(
            "b", "a", "m", "E", "x", None,
        ))
        .unwrap();
        assert_eq!(
            err(&with(
                &e,
                "payload",
                Some(json!({ "error": { "code": "E" } }))
            )),
            "error payload: error must have string code and message"
        );
        let h = serde_json::to_value(A2AProtocol::create_heartbeat("a", "b")).unwrap();
        assert_eq!(
            err(&with(&h, "payload", Some(json!({ "status": "dead" })))),
            "heartbeat payload: status must be 'alive'"
        );
    }

    #[test]
    fn progress_must_be_finite() {
        // JSON に Infinity は書けない。serde_json は非有限の f64 を null にするので、
        // 構造体に NaN / Infinity を入れても progress は数値でないとして弾かれる
        let mut p = A2AProtocol::create_progress_message("b", "a", "t", 0.1, "m");
        p.payload = Some(json!({ "task_id": "t", "progress": f64::INFINITY, "message": "m" }));
        assert_eq!(
            A2AProtocol::validate_message(&p).unwrap_err(),
            "progress payload: progress must be a finite number"
        );
    }

    #[test]
    fn round_trips() {
        let m = req_msg();
        let s = A2AProtocol::serialize(&m).unwrap();
        assert!(s.contains("\"type\": \"task_request\""));
        assert_eq!(A2AProtocol::deserialize(&s).unwrap(), m);
    }

    #[test]
    fn serialize_and_deserialize_reject_invalid_input() {
        let mut m = req_msg();
        m.to.clear();
        assert_eq!(
            A2AProtocol::serialize(&m).unwrap_err(),
            "Invalid A2A message: Missing to"
        );
        assert_eq!(
            A2AProtocol::deserialize("{oops").unwrap_err(),
            "Invalid A2A message: malformed JSON"
        );
        assert_eq!(
            A2AProtocol::deserialize("{}").unwrap_err(),
            "Invalid A2A message: Missing message_id"
        );
        let bad_type = with(&req(), "type", Some(json!("x"))).to_string();
        assert_eq!(
            A2AProtocol::deserialize(&bad_type).unwrap_err(),
            "Invalid A2A message: Unknown message type: x"
        );
    }

    #[test]
    fn is_message_mirrors_validation() {
        assert!(A2AProtocol::is_message(&req()));
        assert!(!A2AProtocol::is_message(&json!({})));
    }

    /// TS の Date.parse と揃える境界（V8 で確認できる代表例）
    #[test]
    fn timestamp_boundaries() {
        let ok =
            |ts: &str| A2AProtocol::validate_value(&with(&req(), "timestamp", Some(json!(ts))));
        // Date.parse は空白区切りも受ける
        for ts in [
            "2024-01-01 00:00:00",
            "2024-01-01 00:00:00.123",
            "2024-01-01 00:00:00Z",
            "2024-01-01 09:00:00+09:00",
        ] {
            assert_eq!(ok(ts), Ok(()), "{ts}");
        }
        // Date.parse はうるう秒・範囲外の値を NaN にする
        for ts in [
            "2024-12-31T23:59:60Z",
            "2024-12-31T23:59:60",
            "2024-12-31 23:59:60",
            "2024-12-31T23:59:60.500+09:00",
            "2024-02-30T00:00:00Z",
            "2024-01-01T24:00:01Z",
            "nope",
        ] {
            assert_eq!(
                ok(ts),
                Err("timestamp must be a valid date string".into()),
                "{ts}"
            );
        }
    }

    /// TS #38 のケース: task_result の payload `{}`、parameters 欠落の task_request
    #[test]
    fn rejects_empty_task_result_payload_and_missing_parameters() {
        assert_eq!(
            err(&with(&result_msg(), "payload", Some(json!({})))),
            "task_result payload: task_id must be a non-empty string"
        );
        assert_eq!(
            err(&with(
                &req(),
                "payload",
                Some(json!({ "task_id": "t", "action": "a" }))
            )),
            "task_request payload: parameters must be an object"
        );
    }

    /// 検証（TS と同じ範囲）は通るが、Rust の構造体の型に合わないため deserialize だけ失敗する例。
    /// TS の検証は request の status や metadata.priority の値を見ない。
    #[test]
    fn valid_per_ts_but_rejected_by_deserialize() {
        for (key, v) in [
            ("status", json!("bogus")),
            ("metadata", json!({ "priority": "bogus" })),
        ] {
            let m = with(&req(), key, Some(v));
            assert!(A2AProtocol::is_message(&m), "{key}");
            assert_eq!(A2AProtocol::validate_value(&m), Ok(()));
            let e = A2AProtocol::deserialize(&m.to_string()).unwrap_err();
            assert!(e.starts_with("Invalid A2A message: "), "{e}");
        }
    }
}
