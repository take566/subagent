//! 全エージェント共通の往復ループ（ConversationRunner）
//!
//! - `end_turn` / `stop_sequence`: 終わり
//! - `tool_use`: client tool を順に実行し、全 tool_result を 1 つの user message にまとめて送る
//! - `refusal`: tool を実行せずに終わる（tool_use が途中で切れていることがある）
//! - `max_tokens`: tool_use 付きなら実行せず、max_tokens を 1 回だけ増やして同じ履歴で再送。
//!   本文だけなら truncated で返す
//! - `pause_turn`: assistant を末尾にしたまま再送する。続きの応答は同じ assistant message に足し、
//!   role が続かないようにする
//!
//! 履歴は追記のみ（送った block は書き換えない）。回数・token 予算・deadline で打ち切る。

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Map, Value, json};
use tokio::time::Instant;

use super::client::LlmClient;
use super::config::TokenBudget;
use super::types::{
    Message, MessageRequest, MessageResponse, Role, StopReason, ToolDef, ToolUse, Usage,
};
use crate::error::Result;

/// client tool を実行する
#[async_trait]
pub trait ToolHandler: Send + Sync {
    /// 入力は schema を通ったものだけが来る。`Err` は `is_error: true` の tool_result になる
    async fn call(&self, name: &str, input: &Value) -> std::result::Result<String, String>;
}

/// 打ち切りの条件
#[derive(Debug, Clone)]
pub struct RunnerLimits {
    /// API を呼ぶ回数の上限
    pub max_iterations: u32,
    /// 1 タスクの token 予算
    pub token_budget: Option<Arc<TokenBudget>>,
    /// これを過ぎたら API も tool も打ち切る
    pub deadline: Option<Instant>,
    /// pause_turn の続きを送る回数の上限
    pub max_pause_continuations: u32,
    /// max_tokens を増やす時の上限
    pub max_tokens_ceiling: u32,
}

impl Default for RunnerLimits {
    fn default() -> Self {
        Self {
            max_iterations: 25,
            token_budget: None,
            deadline: None,
            max_pause_continuations: 5,
            max_tokens_ceiling: 64_000,
        }
    }
}

/// ループが止まった理由
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunStop {
    EndTurn,
    StopSequence,
    Refusal,
    /// max_tokens で本文が切れた
    Truncated,
    /// max_tokens を増やしても tool_use が切れた（tool は実行していない）
    MaxTokensWithToolUse,
    MaxIterations,
    TokenBudget,
    Deadline,
    PauseLimit,
    /// 未知の stop_reason など
    Other(String),
}

/// ループの結果
#[derive(Debug, Clone)]
pub struct RunOutcome {
    pub stop: RunStop,
    /// 最後の応答の本文
    pub text: String,
    /// 送った（送る予定だった）履歴
    pub messages: Vec<Message>,
    /// API を呼んだ回数
    pub iterations: u32,
    /// 全応答の usage の合計
    pub usage: Usage,
    pub last_response: Option<MessageResponse>,
}

/// Messages API との往復を回す
pub struct ConversationRunner {
    client: Arc<dyn LlmClient>,
    template: MessageRequest,
    handler: Arc<dyn ToolHandler>,
    limits: RunnerLimits,
}

impl ConversationRunner {
    /// `template` の model / system / tools / thinking 等を毎回使う（messages は無視する）
    pub fn new(
        client: Arc<dyn LlmClient>,
        template: MessageRequest,
        handler: Arc<dyn ToolHandler>,
    ) -> Self {
        Self {
            client,
            template,
            handler,
            limits: RunnerLimits::default(),
        }
    }

    pub fn with_limits(mut self, limits: RunnerLimits) -> Self {
        self.limits = limits;
        self
    }

    /// 最初の messages（user で始まる）からループを回す
    pub async fn run(&self, initial: Vec<Message>) -> Result<RunOutcome> {
        let mut state = State {
            messages: initial,
            iterations: 0,
            usage: Usage::default(),
            last: None,
        };
        let mut max_tokens = self.template.max_tokens;
        let mut bumped = false;
        let mut pauses = 0u32;

        loop {
            if self.past_deadline() {
                return Ok(state.finish(RunStop::Deadline));
            }
            if state.iterations >= self.limits.max_iterations {
                return Ok(state.finish(RunStop::MaxIterations));
            }
            if self.budget_exhausted() {
                return Ok(state.finish(RunStop::TokenBudget));
            }

            let mut req = self.template.clone();
            req.messages = state.messages.clone();
            req.max_tokens = max_tokens;
            let resp = match self.with_deadline(self.client.create_message(&req)).await {
                Some(r) => r?,
                None => return Ok(state.finish(RunStop::Deadline)),
            };
            state.iterations += 1;
            state.add_usage(&resp.usage);
            if let Some(b) = &self.limits.token_budget {
                b.charge(resp.usage.total());
            }

            let tool_uses = resp.tool_uses();
            match resp.stop_reason.clone() {
                StopReason::EndTurn => return Ok(state.append_and_finish(resp, RunStop::EndTurn)),
                StopReason::StopSequence => {
                    return Ok(state.append_and_finish(resp, RunStop::StopSequence));
                }
                StopReason::Refusal => return Ok(state.append_and_finish(resp, RunStop::Refusal)),
                StopReason::MaxTokens if tool_uses.is_empty() => {
                    return Ok(state.append_and_finish(resp, RunStop::Truncated));
                }
                StopReason::MaxTokens => {
                    // 切れた tool_use は実行せず、履歴にも入れずに再送する
                    state.last = Some(resp);
                    if bumped || max_tokens >= self.limits.max_tokens_ceiling {
                        return Ok(state.finish(RunStop::MaxTokensWithToolUse));
                    }
                    bumped = true;
                    max_tokens = max_tokens
                        .saturating_mul(2)
                        .min(self.limits.max_tokens_ceiling);
                }
                StopReason::PauseTurn => {
                    pauses += 1;
                    state.append(resp);
                    if pauses > self.limits.max_pause_continuations {
                        return Ok(state.finish(RunStop::PauseLimit));
                    }
                    if self.budget_exhausted() {
                        return Ok(state.finish(RunStop::TokenBudget));
                    }
                }
                StopReason::ToolUse if tool_uses.is_empty() => {
                    return Ok(state.append_and_finish(
                        resp,
                        RunStop::Other("tool_use without client tool calls".into()),
                    ));
                }
                StopReason::ToolUse => {
                    state.append(resp);
                    if self.budget_exhausted() {
                        return Ok(state.finish(RunStop::TokenBudget));
                    }
                    let mut results = Vec::with_capacity(tool_uses.len());
                    for tu in &tool_uses {
                        match self.with_deadline(self.run_tool(tu)).await {
                            Some(block) => results.push(block),
                            None => return Ok(state.finish(RunStop::Deadline)),
                        }
                    }
                    state.messages.push(Message::tool_results(results));
                }
                StopReason::Other(reason) => {
                    return Ok(state.append_and_finish(resp, RunStop::Other(reason)));
                }
            }
        }
    }

    fn past_deadline(&self) -> bool {
        self.limits.deadline.is_some_and(|d| Instant::now() >= d)
    }

    fn budget_exhausted(&self) -> bool {
        self.limits
            .token_budget
            .as_ref()
            .is_some_and(|b| b.check().is_err())
    }

    async fn with_deadline<F: std::future::Future>(&self, fut: F) -> Option<F::Output> {
        match self.limits.deadline {
            Some(d) => tokio::time::timeout_at(d, fut).await.ok(),
            None => Some(fut.await),
        }
    }

    /// 1 個の tool_use を検証して実行し、tool_result block を返す
    async fn run_tool(&self, tu: &ToolUse) -> Value {
        let Some(def) = self
            .template
            .tools
            .iter()
            .find(|t| !t.is_server_tool() && t.name() == tu.name)
        else {
            return Message::tool_result_block(&tu.id, format!("unknown tool: {}", tu.name), true);
        };
        if let Err(reason) = validate_input(def, &tu.input) {
            let content = json!({"error": "invalid tool input", "tool": tu.name, "reason": reason});
            return Message::tool_result_block(&tu.id, content.to_string(), true);
        }
        match self.handler.call(&tu.name, &tu.input).await {
            Ok(out) => Message::tool_result_block(&tu.id, out, false),
            Err(err) => Message::tool_result_block(&tu.id, err, true),
        }
    }
}

struct State {
    messages: Vec<Message>,
    iterations: u32,
    usage: Usage,
    last: Option<MessageResponse>,
}

impl State {
    fn add_usage(&mut self, u: &Usage) {
        self.usage.input_tokens += u.input_tokens;
        self.usage.output_tokens += u.output_tokens;
        self.usage.cache_creation_input_tokens += u.cache_creation_input_tokens;
        self.usage.cache_read_input_tokens += u.cache_read_input_tokens;
    }

    /// assistant の応答を履歴に足す。直前が assistant（pause_turn の続き）なら同じ message に足す
    fn append(&mut self, resp: MessageResponse) {
        match self.messages.last_mut() {
            Some(last) if last.role == Role::Assistant => {
                last.content.extend(resp.content.iter().cloned())
            }
            _ => self.messages.push(resp.to_assistant_message()),
        }
        self.last = Some(resp);
    }

    fn append_and_finish(mut self, resp: MessageResponse, stop: RunStop) -> RunOutcome {
        self.append(resp);
        self.finish(stop)
    }

    fn finish(self, stop: RunStop) -> RunOutcome {
        RunOutcome {
            stop,
            text: self.last.as_ref().map(|r| r.text()).unwrap_or_default(),
            messages: self.messages,
            iterations: self.iterations,
            usage: self.usage,
            last_response: self.last,
        }
    }
}

/// tool 入力の簡易検証（object・required・1 段目の type / enum・additionalProperties:false）
fn validate_input(def: &ToolDef, input: &Value) -> std::result::Result<(), String> {
    let Some(obj) = input.as_object() else {
        return Err("input must be a JSON object".into());
    };
    let ToolDef::Custom { input_schema, .. } = def else {
        return Ok(());
    };
    let empty = Map::new();
    let props = input_schema
        .get("properties")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    if let Some(required) = input_schema.get("required").and_then(Value::as_array) {
        for key in required.iter().filter_map(Value::as_str) {
            if !obj.contains_key(key) {
                return Err(format!("missing required field `{key}`"));
            }
        }
    }
    if input_schema.get("additionalProperties") == Some(&Value::Bool(false))
        && let Some(extra) = obj.keys().find(|k| !props.contains_key(*k))
    {
        return Err(format!("unexpected field `{extra}`"));
    }
    for (key, value) in obj {
        let Some(prop) = props.get(key) else { continue };
        if let Some(ty) = prop.get("type") {
            let ok = match ty {
                Value::String(t) => type_matches(t, value),
                Value::Array(ts) => ts
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|t| type_matches(t, value)),
                _ => true,
            };
            if !ok {
                return Err(format!("field `{key}` must be of type {ty}"));
            }
        }
        if let Some(allowed) = prop.get("enum").and_then(Value::as_array)
            && !allowed.contains(value)
        {
            return Err(format!(
                "field `{key}` must be one of {}",
                Value::from(allowed.clone())
            ));
        }
    }
    Ok(())
}

fn type_matches(ty: &str, v: &Value) -> bool {
    match ty {
        "string" => v.is_string(),
        "integer" => v.is_i64() || v.is_u64(),
        "number" => v.is_number(),
        "boolean" => v.is_boolean(),
        "array" => v.is_array(),
        "object" => v.is_object(),
        "null" => v.is_null(),
        _ => true,
    }
}
