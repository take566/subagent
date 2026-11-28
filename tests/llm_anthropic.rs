//! AnthropicClient（raw HTTP）のテスト。wiremock を 127.0.0.1 に立てて使い、外には出ない。

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use subagent::llm::{
    AnthropicClient, BudgetScope, LlmClient, LlmConfig, LlmError, Message, MessageRequest,
    StopReason, TokenBudget,
};
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const KEY: &str = "sk-ant-test-0123456789-secret";

fn ok_body(input: u64, output: u64) -> Value {
    json!({
        "id": "msg_1", "type": "message", "role": "assistant", "model": "claude-opus-5-5",
        "content": [{"type": "text", "text": "hi"}],
        "stop_reason": "end_turn", "stop_sequence": null,
        "usage": {"input_tokens": input, "output_tokens": output}
    })
}

fn err_body(kind: &str, message: &str) -> Value {
    json!({"type": "error", "error": {"type": kind, "message": message}, "request_id": "req_1"})
}

fn config(server: &MockServer) -> LlmConfig {
    let mut c = LlmConfig::new(KEY);
    c.base_url = server.uri();
    c.retry_base_delay = Duration::from_millis(5);
    c.request_timeout = Duration::from_secs(5);
    c
}

fn request() -> MessageRequest {
    MessageRequest::new(64, vec![Message::user_text("hello")])
}

async fn client(c: LlmConfig) -> Arc<AnthropicClient> {
    AnthropicClient::new(c).expect("client builds")
}

#[tokio::test]
async fn sends_documented_headers_and_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", KEY))
        .and(header("anthropic-version", "2026-10-21"))
        .and(header("content-type", "application/json"))
        .and(body_partial_json(json!({
            "model": "claude-opus-5-5",
            "max_tokens": 64,
            "thinking": {"type": "adaptive"},
            "cache_control": {"type": "ephemeral"}
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body(3, 4)))
        .expect(1)
        .mount(&server)
        .await;

    let c = client(config(&server)).await;
    let resp = c.create_message(&request()).await.unwrap();
    assert_eq!(resp.text(), "hi");
    assert_eq!(resp.stop_reason, StopReason::EndTurn);
    assert_eq!(c.daily_used(), 7);

    let reqs = server.received_requests().await.unwrap();
    let body: Value = serde_json::from_slice(&reqs[0].body).unwrap();
    for k in ["temperature", "top_p", "top_k", "tool_choice"] {
        assert!(body.get(k).is_none(), "{k} sent");
    }
}

async fn count(server: &MockServer) -> usize {
    server.received_requests().await.unwrap().len()
}

#[tokio::test]
async fn retries_5xx_and_529_then_succeeds() {
    for status in [500u16, 502, 503, 529] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(status).set_body_json(err_body("api_error", "x")))
            .up_to_n_times(2)
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(ok_body(1, 1)))
            .mount(&server)
            .await;
        let c = client(config(&server)).await;
        c.create_message(&request())
            .await
            .unwrap_or_else(|e| panic!("{status}: {e}"));
        assert_eq!(count(&server).await, 3, "status {status}");
    }
}

#[tokio::test]
async fn gives_up_after_max_retries() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(529).set_body_json(err_body("overloaded_error", "busy")),
        )
        .mount(&server)
        .await;
    let mut cfg = config(&server);
    cfg.max_retries = 2;
    let err = client(cfg)
        .await
        .create_message(&request())
        .await
        .unwrap_err();
    match err {
        LlmError::Api {
            status,
            error_type,
            request_id,
            ..
        } => {
            assert_eq!(status, 529);
            assert_eq!(error_type, "overloaded_error");
            assert_eq!(request_id.as_deref(), Some("req_1"));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(count(&server).await, 3);
}

#[tokio::test]
async fn retries_429_after_retry_after() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "1")
                .set_body_json(err_body("rate_limit_error", "slow down")),
        )
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body(1, 1)))
        .mount(&server)
        .await;
    let start = Instant::now();
    client(config(&server))
        .await
        .create_message(&request())
        .await
        .unwrap();
    assert!(
        start.elapsed() >= Duration::from_millis(900),
        "waited {:?}",
        start.elapsed()
    );
    assert_eq!(count(&server).await, 2);
}

#[tokio::test]
async fn does_not_wait_for_retry_after_beyond_the_cap() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "3600")
                .set_body_json(err_body("rate_limit_error", "later")),
        )
        .mount(&server)
        .await;
    let start = Instant::now();
    let err = client(config(&server))
        .await
        .create_message(&request())
        .await
        .unwrap_err();
    assert!(matches!(err, LlmError::Api { status: 429, .. }), "{err:?}");
    assert!(start.elapsed() < Duration::from_secs(5));
    assert_eq!(count(&server).await, 1);
}

#[tokio::test]
async fn does_not_retry_spend_cap_429() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429).set_body_json(json!({
            "type": "error",
            "error": {"type": "rate_limit_error", "message": "spend cap",
                      "details": {"error_code": "enforced_spend_limit_reached"}}
        })))
        .mount(&server)
        .await;
    let err = client(config(&server))
        .await
        .create_message(&request())
        .await
        .unwrap_err();
    assert!(matches!(err, LlmError::Api { status: 429, .. }), "{err:?}");
    assert_eq!(count(&server).await, 1);
}

#[tokio::test]
async fn does_not_retry_client_errors() {
    for (status, kind) in [
        (400u16, "invalid_request_error"),
        (401, "authentication_error"),
        (402, "billing_error"),
        (403, "permission_error"),
        (404, "not_found_error"),
        (413, "request_too_large"),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(status).set_body_json(err_body(kind, "no")))
            .mount(&server)
            .await;
        let err = client(config(&server))
            .await
            .create_message(&request())
            .await
            .unwrap_err();
        match err {
            LlmError::Api {
                status: s,
                error_type,
                ..
            } => {
                assert_eq!(s, status);
                assert_eq!(error_type, kind);
            }
            other => panic!("{status}: {other:?}"),
        }
        assert_eq!(count(&server).await, 1, "status {status} retried");
    }
}

#[tokio::test]
async fn does_not_retry_after_read_timeout() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(ok_body(1, 1))
                .set_delay(Duration::from_secs(3)),
        )
        .mount(&server)
        .await;
    let mut cfg = config(&server);
    cfg.request_timeout = Duration::from_millis(300);
    let c = client(cfg).await;
    let err = c.create_message(&request()).await.unwrap_err();
    // 送った後に切れた分は課金された前提で、見積もりを 1 日の使用量に数える
    match err {
        LlmError::PossiblyBilled {
            estimated_tokens, ..
        } => {
            assert!(estimated_tokens >= 64, "{estimated_tokens}");
            assert_eq!(c.daily_used(), estimated_tokens);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(count(&server).await, 1, "a sent request must not be resent");
}

#[tokio::test]
async fn deadline_bounds_the_request_and_counts_it_as_billed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(ok_body(1, 1))
                .set_delay(Duration::from_secs(5)),
        )
        .mount(&server)
        .await;
    let mut cfg = config(&server);
    cfg.request_timeout = Duration::from_secs(60);
    let c = client(cfg).await;
    let start = Instant::now();
    let deadline = tokio::time::Instant::now() + Duration::from_millis(300);
    let err = c
        .create_message_until(&request(), Some(deadline))
        .await
        .unwrap_err();
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "{:?}",
        start.elapsed()
    );
    assert!(matches!(err, LlmError::PossiblyBilled { .. }), "{err:?}");
    assert!(c.daily_used() > 0);

    // 過ぎた deadline では送らない
    let err = c
        .create_message_until(&request(), Some(tokio::time::Instant::now()))
        .await
        .unwrap_err();
    assert!(matches!(err, LlmError::DeadlineExceeded), "{err:?}");
    assert_eq!(count(&server).await, 1);
}

#[tokio::test]
async fn unparseable_retry_after_is_not_retried_immediately() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "Wed, 21 Oct 2026 07:28:00 GMT")
                .set_body_json(err_body("rate_limit_error", "later")),
        )
        .mount(&server)
        .await;
    let err = client(config(&server))
        .await
        .create_message(&request())
        .await
        .unwrap_err();
    assert!(matches!(err, LlmError::Api { status: 429, .. }), "{err:?}");
    assert_eq!(count(&server).await, 1);
}

#[tokio::test]
async fn retry_after_is_honored_on_5xx_too() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(529)
                .insert_header("retry-after", "1")
                .set_body_json(err_body("overloaded_error", "busy")),
        )
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body(1, 1)))
        .mount(&server)
        .await;
    let start = Instant::now();
    client(config(&server))
        .await
        .create_message(&request())
        .await
        .unwrap();
    assert!(
        start.elapsed() >= Duration::from_millis(900),
        "{:?}",
        start.elapsed()
    );
    assert_eq!(count(&server).await, 2);
}

#[tokio::test]
async fn unbilled_errors_release_the_reservation() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(400).set_body_json(err_body("invalid_request_error", "no")),
        )
        .mount(&server)
        .await;
    let c = client(config(&server)).await;
    let _ = c.create_message(&request()).await.unwrap_err();
    assert_eq!(c.daily_used(), 0);
}

#[tokio::test]
async fn retries_connect_errors() {
    // 閉じた port: 接続が拒否される
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut cfg = LlmConfig::new(KEY);
    cfg.base_url = format!("http://127.0.0.1:{port}");
    cfg.retry_base_delay = Duration::from_millis(1);
    cfg.max_retries = 2;
    let err = client(cfg)
        .await
        .create_message(&request())
        .await
        .unwrap_err();
    match err {
        LlmError::Transport { attempts, message } => {
            assert_eq!(attempts, 3);
            assert!(!message.contains(KEY));
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn semaphore_limits_concurrency() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(ok_body(1, 1))
                .set_delay(Duration::from_millis(200)),
        )
        .mount(&server)
        .await;
    let mut cfg = config(&server);
    cfg.max_concurrency = 2;
    let c = client(cfg).await;
    let start = Instant::now();
    let req = request();
    let calls = (0..6).map(|_| c.create_message(&req));
    for r in futures::future::join_all(calls).await {
        r.unwrap();
    }
    // 2 並列で 6 本 → 少なくとも 3 回分の遅延
    assert!(
        start.elapsed() >= Duration::from_millis(590),
        "{:?}",
        start.elapsed()
    );
}

#[tokio::test]
async fn daily_cap_blocks_further_calls_without_sending() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body(80, 40)))
        .expect(1)
        .mount(&server)
        .await;
    let mut cfg = config(&server);
    cfg.max_tokens_per_day = 100;
    let c = client(cfg).await;
    c.create_message(&request()).await.unwrap();
    let err = c.create_message(&request()).await.unwrap_err();
    match err {
        LlmError::BudgetExceeded { scope, used, limit } => {
            assert_eq!(scope, BudgetScope::Daily);
            assert_eq!(used, 120);
            assert_eq!(limit, 100);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(count(&server).await, 1);
}

#[tokio::test]
async fn daily_cap_holds_under_concurrent_calls() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(ok_body(80, 40))
                .set_delay(Duration::from_millis(50)),
        )
        .mount(&server)
        .await;
    let mut cfg = config(&server);
    cfg.max_concurrency = 1;
    cfg.max_tokens_per_day = 100;
    let c = client(cfg).await;
    let req = request();
    let results = futures::future::join_all((0..8).map(|_| c.create_message(&req))).await;
    let ok = results.iter().filter(|r| r.is_ok()).count();
    assert_eq!(ok, 1, "{results:?}");
    for r in results.iter().filter(|r| r.is_err()) {
        assert!(
            matches!(
                r,
                Err(LlmError::BudgetExceeded {
                    scope: BudgetScope::Daily,
                    ..
                })
            ),
            "{r:?}"
        );
    }
    assert_eq!(
        count(&server).await,
        1,
        "requests queued behind the permit must re-check the cap"
    );
    assert_eq!(c.daily_used(), 120);
}

#[tokio::test]
async fn reservations_block_parallel_calls_from_overshooting() {
    // 同時 4 本でも、予約（max_tokens + 入力の見積もり）で上限を超えて送らない
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(ok_body(80, 40))
                .set_delay(Duration::from_millis(100)),
        )
        .mount(&server)
        .await;
    let mut cfg = config(&server);
    cfg.max_concurrency = 4;
    cfg.max_tokens_per_day = 100;
    let c = client(cfg).await;
    let req = request();
    let _ = futures::future::join_all((0..8).map(|_| c.create_message(&req))).await;
    assert_eq!(count(&server).await, 1);
}

#[tokio::test]
async fn garbage_200_is_counted_as_possibly_billed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
        .mount(&server)
        .await;
    let c = client(config(&server)).await;
    let err = c.create_message(&request()).await.unwrap_err();
    match err {
        LlmError::PossiblyBilled {
            reason,
            estimated_tokens,
        } => {
            assert!(reason.contains("decode"), "{reason}");
            assert_eq!(c.daily_used(), estimated_tokens);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(count(&server).await, 1);
}

#[tokio::test]
async fn api_key_never_appears_in_debug_or_errors() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|req: &Request| {
            // 届いた鍵は header だけ
            assert!(!String::from_utf8_lossy(&req.body).contains(KEY));
            ResponseTemplate::new(401).set_body_json(err_body("authentication_error", "bad key"))
        })
        .mount(&server)
        .await;
    let cfg = config(&server);
    assert!(!format!("{cfg:?}").contains(KEY), "{cfg:?}");
    let c = client(cfg).await;
    assert!(!format!("{c:?}").contains(KEY), "{c:?}");
    let err = c.create_message(&request()).await.unwrap_err();
    assert!(!format!("{err} {err:?}").contains(KEY));
}

#[test]
fn config_from_lookup_prefers_subagent_key_and_reads_daily_cap() {
    let env = |pairs: &'static [(&'static str, &'static str)]| {
        move |k: &str| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    };
    let c = LlmConfig::from_lookup(env(&[
        ("SUBAGENT_ANTHROPIC_API_KEY", "sub-key"),
        ("ANTHROPIC_API_KEY", "generic-key"),
    ]))
    .unwrap()
    .unwrap();
    assert_eq!(c.api_key.expose(), "sub-key");
    assert_eq!(c.max_tokens_per_day, 2_000_000);

    let c = LlmConfig::from_lookup(env(&[
        ("SUBAGENT_ANTHROPIC_API_KEY", ""),
        ("ANTHROPIC_API_KEY", "generic-key"),
        ("SUBAGENT_MAX_TOKENS_PER_DAY", "5000"),
    ]))
    .unwrap()
    .unwrap();
    assert_eq!(c.api_key.expose(), "generic-key");
    assert_eq!(c.max_tokens_per_day, 5000);
    assert_eq!(c.base_url, "https://api.anthropic.com");

    assert!(LlmConfig::from_lookup(env(&[])).unwrap().is_none());
    assert!(
        LlmConfig::from_lookup(env(&[
            ("ANTHROPIC_API_KEY", "k"),
            ("SUBAGENT_MAX_TOKENS_PER_DAY", "lots")
        ]))
        .is_err()
    );
}

#[test]
fn task_budget_defaults_and_charging() {
    assert_eq!(TokenBudget::research().limit(), 300_000);
    assert_eq!(TokenBudget::codegen().limit(), 1_000_000);
    assert_eq!(TokenBudget::review().limit(), 150_000);
    assert_eq!(TokenBudget::document().limit(), 100_000);

    let b = TokenBudget::new(100);
    assert!(b.check().is_ok());
    b.charge(60);
    assert_eq!(b.remaining(), 40);
    b.charge(60);
    assert_eq!(b.used(), 120);
    match b.check().unwrap_err() {
        LlmError::BudgetExceeded { scope, used, limit } => {
            assert_eq!(scope, BudgetScope::Task);
            assert_eq!((used, limit), (120, 100));
        }
        other => panic!("{other:?}"),
    }
}
