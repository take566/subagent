//! runner の送信前の予算確認（critic ruling 12）: 残りの予算が「request の見積もり + max_tokens」より
//! 少なければ送らずに TokenBudget で止まる。`RunnerLimits::check_budget_before_send` を立てた時だけ

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use subagent::llm::{
    ConversationRunner, Message, MessageRequest, RunStop, RunnerLimits, ScriptedLlm, StopReason,
    TokenBudget, ToolHandler, estimate_request_tokens,
};

struct NoTools;

#[async_trait]
impl ToolHandler for NoTools {
    async fn call(&self, _name: &str, _input: &Value) -> Result<String, String> {
        Err("no client tools".into())
    }
}

fn template(max_tokens: u32) -> MessageRequest {
    MessageRequest::new(max_tokens, vec![])
}

fn pause(text: &str) -> subagent::llm::MessageResponse {
    ScriptedLlm::response(
        StopReason::PauseTurn,
        vec![json!({"type": "text", "text": text})],
    )
}

fn limits(budget: &Arc<TokenBudget>) -> RunnerLimits {
    RunnerLimits {
        token_budget: Some(budget.clone()),
        check_budget_before_send: true,
        ..RunnerLimits::default()
    }
}

#[test]
fn estimate_is_serialized_bytes_over_three_rounded_up_plus_max_tokens() {
    let mut req = template(100);
    req.messages = vec![Message::user_text("x".repeat(400))];
    let bytes = serde_json::to_vec(&req).unwrap().len() as u64;
    assert_eq!(estimate_request_tokens(&req), bytes.div_ceil(3) + 100);
}

#[tokio::test]
async fn refuses_the_first_request_when_max_tokens_alone_exceeds_the_budget() {
    let llm = Arc::new(ScriptedLlm::new(vec![Ok(ScriptedLlm::text_response("x"))]));
    let budget = Arc::new(TokenBudget::new(500));
    let out = ConversationRunner::new(llm.clone(), template(1000), Arc::new(NoTools))
        .with_limits(limits(&budget))
        .run(vec![Message::user_text("go")])
        .await
        .unwrap();
    assert_eq!(out.stop, RunStop::TokenBudget);
    assert!(llm.requests().is_empty(), "nothing must be sent");
    assert_eq!(budget.used(), 0);
}

#[tokio::test]
async fn refuses_a_later_request_that_no_longer_fits() {
    // 1 応答 20 token。1 本目は収まるが、2 本目の時点の残りでは max_tokens 分が足りない
    let llm = Arc::new(ScriptedLlm::new(vec![
        Ok(pause("a")),
        Ok(ScriptedLlm::text_response("b")),
    ]));
    let first = {
        let mut r = template(200);
        r.messages = vec![Message::user_text("go")];
        estimate_request_tokens(&r)
    };
    // 1 本目はぴったり収まり、20 token 使った後は足りない
    let budget = Arc::new(TokenBudget::new(first + 10));
    let out = ConversationRunner::new(llm.clone(), template(200), Arc::new(NoTools))
        .with_limits(limits(&budget))
        .run(vec![Message::user_text("go")])
        .await
        .unwrap();
    assert_eq!(out.stop, RunStop::TokenBudget);
    assert_eq!(llm.requests().len(), 1);
    assert_eq!(out.iterations, 1);
    assert_eq!(out.text, "a", "the paused content is kept");
}

#[tokio::test]
async fn sends_when_the_estimate_fits() {
    let llm = Arc::new(ScriptedLlm::new(vec![Ok(ScriptedLlm::text_response("ok"))]));
    let budget = Arc::new(TokenBudget::new(100_000));
    let out = ConversationRunner::new(llm.clone(), template(1000), Arc::new(NoTools))
        .with_limits(limits(&budget))
        .run(vec![Message::user_text("go")])
        .await
        .unwrap();
    assert_eq!(out.stop, RunStop::EndTurn);
    assert_eq!(llm.requests().len(), 1);
}

/// 既定（false）は従来どおり、使い切るまで送る（他のエージェントの挙動を変えない）
#[tokio::test]
async fn off_by_default() {
    assert!(!RunnerLimits::default().check_budget_before_send);
    let llm = Arc::new(ScriptedLlm::new(vec![Ok(ScriptedLlm::text_response("x"))]));
    let budget = Arc::new(TokenBudget::new(500));
    let out = ConversationRunner::new(llm.clone(), template(1000), Arc::new(NoTools))
        .with_limits(RunnerLimits {
            token_budget: Some(budget),
            ..RunnerLimits::default()
        })
        .run(vec![Message::user_text("go")])
        .await
        .unwrap();
    assert_eq!(out.stop, RunStop::EndTurn);
    assert_eq!(llm.requests().len(), 1);
}

/// per_request_reserve（server tool が足す入力の見込み）も見積もりに足す
#[tokio::test]
async fn per_request_reserve_is_added_to_the_estimate() {
    let mut req = template(100);
    req.messages = vec![Message::user_text("go")];
    let need = estimate_request_tokens(&req);
    assert_eq!(RunnerLimits::default().per_request_reserve, 0);
    for (reserve, sent) in [(0, 1), (1, 0)] {
        let llm = Arc::new(ScriptedLlm::new(vec![Ok(ScriptedLlm::text_response("x"))]));
        let budget = Arc::new(TokenBudget::new(need));
        let out = ConversationRunner::new(llm.clone(), template(100), Arc::new(NoTools))
            .with_limits(RunnerLimits {
                per_request_reserve: reserve,
                ..limits(&budget)
            })
            .run(vec![Message::user_text("go")])
            .await
            .unwrap();
        assert_eq!(llm.requests().len(), sent, "reserve {reserve}");
        if sent == 0 {
            assert_eq!(out.stop, RunStop::TokenBudget);
        }
    }
}

/// エージェントの上限（AgentLlmSettings::runner_limits）は全エージェントで送る前に確かめる
#[test]
fn agent_runner_limits_check_the_budget_before_sending() {
    use subagent::llm::{AgentKind, AgentLlmSettings};
    for kind in [
        AgentKind::Research,
        AgentKind::CodeGen,
        AgentKind::Review,
        AgentKind::Document,
    ] {
        let l = AgentLlmSettings::defaults(kind).runner_limits(tokio::time::Instant::now());
        assert!(l.check_budget_before_send, "{kind:?}");
    }
}
