//! ConversationRunner のテスト（ScriptedLlm を使い、通信しない）

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use subagent::llm::{
    ConversationRunner, LlmError, Message, MessageRequest, RunStop, RunnerLimits, ScriptedLlm,
    StopReason, TokenBudget, ToolDef, ToolHandler,
};

#[derive(Default)]
struct Recorder {
    calls: Mutex<Vec<(String, Value)>>,
}

impl Recorder {
    fn calls(&self) -> Vec<(String, Value)> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl ToolHandler for Recorder {
    async fn call(&self, name: &str, input: &Value) -> Result<String, String> {
        self.calls
            .lock()
            .unwrap()
            .push((name.to_string(), input.clone()));
        match name {
            "fail" => Err("boom".into()),
            "slow" => {
                tokio::time::sleep(Duration::from_secs(5)).await;
                Ok("late".into())
            }
            _ => Ok(format!("ok:{}", input["q"].as_str().unwrap_or(""))),
        }
    }
}

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {"q": {"type": "string"}},
        "required": ["q"],
        "additionalProperties": false
    })
}

fn template() -> MessageRequest {
    let mut t = MessageRequest::new(1000, vec![]);
    t.tools = vec![
        ToolDef::custom("lookup", "look up", schema()),
        ToolDef::custom("fail", "always fails", schema()),
        ToolDef::custom("slow", "slow", schema()),
        ToolDef::web_search(),
    ];
    t
}

fn tool_use(id: &str, name: &str, input: Value) -> Value {
    json!({"type": "tool_use", "id": id, "name": name, "input": input})
}

fn text(t: &str) -> Value {
    json!({"type": "text", "text": t})
}

fn resp(stop: StopReason, content: Vec<Value>) -> Result<subagent::llm::MessageResponse, LlmError> {
    Ok(ScriptedLlm::response(stop, content))
}

fn runner(llm: Arc<ScriptedLlm>, rec: Arc<Recorder>, limits: RunnerLimits) -> ConversationRunner {
    ConversationRunner::new(llm, template(), rec).with_limits(limits)
}

fn roles(req: &Value) -> Vec<String> {
    req["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["role"].as_str().unwrap().to_string())
        .collect()
}

fn assert_alternates(req: &Value) {
    let r = roles(req);
    assert_eq!(r[0], "user");
    for w in r.windows(2) {
        assert_ne!(w[0], w[1], "duplicate role in {r:?}");
    }
}

// 12. 全 tool_result を 1 つの user message に、id を対応させて入れる
#[tokio::test]
async fn tool_results_batched_in_one_user_message_with_matching_ids() {
    let llm = Arc::new(ScriptedLlm::new(vec![
        resp(
            StopReason::ToolUse,
            vec![
                text("checking"),
                tool_use("toolu_a", "lookup", json!({"q": "a"})),
                tool_use("toolu_b", "lookup", json!({"q": "b"})),
            ],
        ),
        Ok(ScriptedLlm::text_response("done")),
    ]));
    let rec = Arc::new(Recorder::default());
    let out = runner(llm.clone(), rec.clone(), RunnerLimits::default())
        .run(vec![Message::user_text("go")])
        .await
        .unwrap();
    assert_eq!(out.stop, RunStop::EndTurn);
    assert_eq!(out.text, "done");
    assert_eq!(out.iterations, 2);
    assert_eq!(rec.calls().len(), 2);

    let reqs = llm.requests();
    assert_eq!(reqs.len(), 2);
    let msgs = reqs[1]["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 3);
    assert_eq!(msgs[1]["role"], "assistant");
    assert_eq!(
        msgs[2],
        json!({"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "toolu_a", "content": "ok:a"},
            {"type": "tool_result", "tool_use_id": "toolu_b", "content": "ok:b"}
        ]})
    );
    // 履歴は追記のみ: 1 本目の messages は 2 本目の先頭と同じ
    assert_eq!(reqs[0]["messages"][0], reqs[1]["messages"][0]);
    assert_alternates(&reqs[1]);
    assert_eq!(out.messages.len(), 4);
    assert_eq!(out.usage.total(), 40);
}

// 13. pause_turn は assistant を末尾にして再送し、続きは同じ assistant message に足す
#[tokio::test]
async fn pause_turn_resends_with_assistant_last_and_never_duplicates_roles() {
    let paused = vec![
        json!({"type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search", "input": {"query": "x"}}),
    ];
    let llm = Arc::new(ScriptedLlm::new(vec![
        resp(StopReason::PauseTurn, paused.clone()),
        resp(
            StopReason::ToolUse,
            vec![
                text("found"),
                tool_use("toolu_1", "lookup", json!({"q": "z"})),
            ],
        ),
        Ok(ScriptedLlm::text_response("final")),
    ]));
    let rec = Arc::new(Recorder::default());
    let out = runner(llm.clone(), rec.clone(), RunnerLimits::default())
        .run(vec![Message::user_text("search")])
        .await
        .unwrap();
    assert_eq!(out.stop, RunStop::EndTurn);
    let reqs = llm.requests();
    assert_eq!(reqs.len(), 3);

    // 2 本目: user, assistant（paused content そのまま）で、余計な user を足さない
    assert_eq!(roles(&reqs[1]), ["user", "assistant"]);
    assert_eq!(reqs[1]["messages"][1]["content"], json!(paused));

    // 3 本目: 続きは同じ assistant に足され、role は交互
    assert_eq!(roles(&reqs[2]), ["user", "assistant", "user"]);
    assert_alternates(&reqs[2]);
    let merged = reqs[2]["messages"][1]["content"].as_array().unwrap();
    assert_eq!(merged.len(), 3);
    assert_eq!(merged[0], paused[0]);
    assert_eq!(
        reqs[2]["messages"][2]["content"][0]["tool_use_id"],
        "toolu_1"
    );
}

#[tokio::test]
async fn pause_turn_continuations_are_limited() {
    let pause = || {
        resp(
            StopReason::PauseTurn,
            vec![json!({"type": "server_tool_use", "id": "s", "name": "web_search", "input": {}})],
        )
    };
    let llm = Arc::new(ScriptedLlm::new(vec![pause(), pause(), pause()]));
    let limits = RunnerLimits {
        max_pause_continuations: 1,
        ..RunnerLimits::default()
    };
    let out = runner(llm.clone(), Arc::new(Recorder::default()), limits)
        .run(vec![Message::user_text("s")])
        .await
        .unwrap();
    assert_eq!(out.stop, RunStop::PauseLimit);
    assert_eq!(llm.requests().len(), 2);
}

// 14. refusal では tool を実行しない
#[tokio::test]
async fn refusal_runs_no_tools() {
    let llm = Arc::new(ScriptedLlm::new(vec![resp(
        StopReason::Refusal,
        vec![tool_use("toolu_x", "lookup", json!({"q": "partial"}))],
    )]));
    let rec = Arc::new(Recorder::default());
    let out = runner(llm.clone(), rec.clone(), RunnerLimits::default())
        .run(vec![Message::user_text("bad")])
        .await
        .unwrap();
    assert_eq!(out.stop, RunStop::Refusal);
    assert!(rec.calls().is_empty());
    assert_eq!(llm.requests().len(), 1);
}

// 15. max_tokens で tool_use 付き: 実行せず max_tokens を 1 回だけ増やして再送
#[tokio::test]
async fn max_tokens_with_tool_use_retries_once_with_more_tokens_then_stops() {
    let truncated = || {
        resp(
            StopReason::MaxTokens,
            vec![tool_use("toolu_t", "lookup", json!({"q": "tru"}))],
        )
    };
    let llm = Arc::new(ScriptedLlm::new(vec![truncated(), truncated()]));
    let rec = Arc::new(Recorder::default());
    let out = runner(llm.clone(), rec.clone(), RunnerLimits::default())
        .run(vec![Message::user_text("go")])
        .await
        .unwrap();
    assert_eq!(out.stop, RunStop::MaxTokensWithToolUse);
    assert!(rec.calls().is_empty(), "truncated tool input must not run");
    let reqs = llm.requests();
    assert_eq!(reqs.len(), 2);
    assert_eq!(reqs[0]["max_tokens"], 1000);
    assert_eq!(reqs[1]["max_tokens"], 2000);
    // 切れた assistant は履歴に入れずに再送する
    assert_eq!(reqs[1]["messages"], reqs[0]["messages"]);
}

#[tokio::test]
async fn max_tokens_retry_that_succeeds_continues_normally() {
    let llm = Arc::new(ScriptedLlm::new(vec![
        resp(
            StopReason::MaxTokens,
            vec![tool_use("toolu_t", "lookup", json!({"q": "tr"}))],
        ),
        resp(
            StopReason::ToolUse,
            vec![tool_use("toolu_u", "lookup", json!({"q": "full"}))],
        ),
        Ok(ScriptedLlm::text_response("ok")),
    ]));
    let rec = Arc::new(Recorder::default());
    let out = runner(llm.clone(), rec.clone(), RunnerLimits::default())
        .run(vec![Message::user_text("go")])
        .await
        .unwrap();
    assert_eq!(out.stop, RunStop::EndTurn);
    assert_eq!(rec.calls().len(), 1);
    assert_eq!(rec.calls()[0].1, json!({"q": "full"}));
}

// 16. max_tokens で本文だけなら truncated で返す
#[tokio::test]
async fn max_tokens_text_only_returns_truncated() {
    let llm = Arc::new(ScriptedLlm::new(vec![resp(
        StopReason::MaxTokens,
        vec![text("partial answ")],
    )]));
    let out = runner(
        llm.clone(),
        Arc::new(Recorder::default()),
        RunnerLimits::default(),
    )
    .run(vec![Message::user_text("go")])
    .await
    .unwrap();
    assert_eq!(out.stop, RunStop::Truncated);
    assert_eq!(out.text, "partial answ");
    assert_eq!(llm.requests().len(), 1);
}

fn looping_script(n: usize) -> Vec<Result<subagent::llm::MessageResponse, LlmError>> {
    (0..n)
        .map(|i| {
            resp(
                StopReason::ToolUse,
                vec![tool_use(&format!("toolu_{i}"), "lookup", json!({"q": "x"}))],
            )
        })
        .collect()
}

// 17. 回数の上限
#[tokio::test]
async fn iteration_limit_stops_the_loop() {
    let llm = Arc::new(ScriptedLlm::new(looping_script(10)));
    let limits = RunnerLimits {
        max_iterations: 3,
        ..RunnerLimits::default()
    };
    let out = runner(llm.clone(), Arc::new(Recorder::default()), limits)
        .run(vec![Message::user_text("go")])
        .await
        .unwrap();
    assert_eq!(out.stop, RunStop::MaxIterations);
    assert_eq!(out.iterations, 3);
    assert_eq!(llm.requests().len(), 3);
}

// 18. token 予算
#[tokio::test]
async fn token_budget_stops_before_running_more_tools() {
    // 1 応答 20 token、予算 30 → 2 本目の応答で使い切り、その tool は動かさない
    let llm = Arc::new(ScriptedLlm::new(looping_script(10)));
    let budget = Arc::new(TokenBudget::new(30));
    let rec = Arc::new(Recorder::default());
    let limits = RunnerLimits {
        token_budget: Some(budget.clone()),
        ..RunnerLimits::default()
    };
    let out = runner(llm.clone(), rec.clone(), limits)
        .run(vec![Message::user_text("go")])
        .await
        .unwrap();
    assert_eq!(out.stop, RunStop::TokenBudget);
    assert_eq!(llm.requests().len(), 2);
    assert_eq!(rec.calls().len(), 1);
    assert_eq!(budget.used(), 40);
}

#[tokio::test]
async fn exhausted_budget_sends_nothing() {
    let llm = Arc::new(ScriptedLlm::new(looping_script(1)));
    let budget = Arc::new(TokenBudget::new(10));
    budget.charge(10);
    let limits = RunnerLimits {
        token_budget: Some(budget),
        ..RunnerLimits::default()
    };
    let out = runner(llm.clone(), Arc::new(Recorder::default()), limits)
        .run(vec![Message::user_text("go")])
        .await
        .unwrap();
    assert_eq!(out.stop, RunStop::TokenBudget);
    assert!(llm.requests().is_empty());
}

// 19. deadline
#[tokio::test]
async fn deadline_already_passed_sends_nothing() {
    let llm = Arc::new(ScriptedLlm::new(looping_script(1)));
    let limits = RunnerLimits {
        deadline: Some(tokio::time::Instant::now()),
        ..RunnerLimits::default()
    };
    let out = runner(llm.clone(), Arc::new(Recorder::default()), limits)
        .run(vec![Message::user_text("go")])
        .await
        .unwrap();
    assert_eq!(out.stop, RunStop::Deadline);
    assert!(llm.requests().is_empty());
}

#[tokio::test]
async fn deadline_interrupts_a_slow_tool() {
    let llm = Arc::new(ScriptedLlm::new(vec![
        resp(
            StopReason::ToolUse,
            vec![tool_use("toolu_s", "slow", json!({"q": "s"}))],
        ),
        Ok(ScriptedLlm::text_response("never")),
    ]));
    let limits = RunnerLimits {
        deadline: Some(tokio::time::Instant::now() + Duration::from_millis(200)),
        ..RunnerLimits::default()
    };
    let start = std::time::Instant::now();
    let out = runner(llm.clone(), Arc::new(Recorder::default()), limits)
        .run(vec![Message::user_text("go")])
        .await
        .unwrap();
    assert_eq!(out.stop, RunStop::Deadline);
    assert!(start.elapsed() < Duration::from_secs(3));
    assert_eq!(llm.requests().len(), 1);
}

// 20. tool のエラーと不正な入力は is_error の tool_result
#[tokio::test]
async fn tool_errors_and_invalid_input_become_is_error_results() {
    let llm = Arc::new(ScriptedLlm::new(vec![
        resp(
            StopReason::ToolUse,
            vec![
                tool_use("toolu_f", "fail", json!({"q": "x"})),
                tool_use("toolu_m", "lookup", json!({})),
                tool_use("toolu_w", "lookup", json!({"q": 5})),
                tool_use("toolu_e", "lookup", json!({"q": "x", "extra": true})),
                tool_use("toolu_s", "lookup", json!("not an object")),
                tool_use("toolu_u", "no_such_tool", json!({"q": "x"})),
                tool_use("toolu_ok", "lookup", json!({"q": "fine"})),
            ],
        ),
        Ok(ScriptedLlm::text_response("recovered")),
    ]));
    let rec = Arc::new(Recorder::default());
    let out = runner(llm.clone(), rec.clone(), RunnerLimits::default())
        .run(vec![Message::user_text("go")])
        .await
        .unwrap();
    assert_eq!(out.stop, RunStop::EndTurn);
    // 不正な入力・未知の tool は handler を呼ばない
    let names: Vec<_> = rec.calls().into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, ["fail", "lookup"]);

    let reqs = llm.requests();
    let results = reqs[1]["messages"][2]["content"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(results.len(), 7);
    let ids: Vec<_> = results
        .iter()
        .map(|r| r["tool_use_id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "toolu_f", "toolu_m", "toolu_w", "toolu_e", "toolu_s", "toolu_u", "toolu_ok"
        ]
    );
    for r in &results[..6] {
        assert_eq!(r["is_error"], true, "{r}");
    }
    assert!(results[6].get("is_error").is_none());
    assert_eq!(results[0]["content"], "boom");
    assert_eq!(results[6]["content"], "ok:fine");
}

#[tokio::test]
async fn llm_errors_propagate() {
    let llm = Arc::new(ScriptedLlm::new(vec![Err(LlmError::Api {
        status: 400,
        error_type: "invalid_request_error".into(),
        message: "bad".into(),
        request_id: None,
    })]));
    let err = runner(llm, Arc::new(Recorder::default()), RunnerLimits::default())
        .run(vec![Message::user_text("go")])
        .await
        .unwrap_err();
    assert_eq!(err.name(), "LlmError");
}
