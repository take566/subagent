//! 全エージェント共通の往復ループ（ConversationRunner）
//!
//! - `end_turn` / `stop_sequence`: 終わり
//! - `tool_use`: client tool を順に実行し、全 tool_result を 1 つの user message にまとめて送る
//! - `refusal`: tool を実行せずに終わる（tool_use が途中で切れていることがある）
//! - `max_tokens`: tool_use 付きなら実行せず、max_tokens を増やして同じ履歴で再送。
//!   増やすのは 1 回の run で 1 回だけで、増やした値はその run の残りの request でも使い続ける。
//!   本文だけなら truncated で返す
//! - `pause_turn`: assistant を末尾にしたまま再送する。続きの応答は同じ assistant message に足し、
//!   role が続かないようにする
//!
//! 履歴は追記のみ（送った block は書き換えない）。回数・token 予算・deadline で打ち切る。
//!
//! deadline を過ぎると、実行中の tool の future はその場で drop される。tool の実装は
//! cancel-safe にすること（子 process は `kill_on_drop(true)` にする、書き込みは一時 file から
//! rename する、など）。LLM 呼び出しは [`LlmClient::create_message_until`] に deadline を渡す。
//!
//! LLM のエラーでも `Err` にはせず、それまでの回数・usage・履歴を [`RunStop::LlmError`] と
//! [`RunOutcome::error`] で返す。課金された可能性があるエラー（`PossiblyBilled`）は
//! その見積もりを 1 タスクの予算に数える。

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Map, Value, json};
use tokio::time::Instant;

use super::client::{LlmClient, LlmError};
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

/// 往復ごとに呼ばれる（MCP の進捗通知に使う。[`crate::core::ProgressSink`] が実装する）
#[async_trait]
pub trait RunObserver: Send + Sync {
    /// API の応答を 1 つ受け取るたびに呼ばれる（`iteration` は 1 始まり）
    async fn on_iteration(&self, iteration: u32, response: &MessageResponse);
}

/// 打ち切りの条件
#[derive(Debug, Clone)]
pub struct RunnerLimits {
    /// API を呼ぶ回数の上限
    pub max_iterations: u32,
    /// 1 タスクの token 予算
    pub token_budget: Option<Arc<TokenBudget>>,
    /// これを過ぎたら API も tool も打ち切る（実行中の tool の future は drop される）
    pub deadline: Option<Instant>,
    /// pause_turn の続きを送る回数の上限
    pub max_pause_continuations: u32,
    /// max_tokens を増やす時の上限（streaming しない request なので既定は 32000）
    pub max_tokens_ceiling: u32,
    /// 送る前に予算を確かめる: 残りが [`estimate_request_tokens`]（入力の概算 + max_tokens）+
    /// [`Self::per_request_reserve`] より少なければ送らずに [`RunStop::TokenBudget`] で止める。
    /// 既定は false（使い切るまで送る）。エージェントの [`crate::llm::AgentLlmSettings::runner_limits`] は true
    pub check_budget_before_send: bool,
    /// 送る前の確認で 1 request ごとに足す token 数（server tool の結果が入力に足される見込みなど）。既定 0
    pub per_request_reserve: u64,
}

/// streaming しない request で max_tokens を増やす時の既定の上限
pub const DEFAULT_MAX_TOKENS_CEILING: u32 = 32_000;

impl Default for RunnerLimits {
    fn default() -> Self {
        Self {
            max_iterations: 25,
            token_budget: None,
            deadline: None,
            max_pause_continuations: 5,
            max_tokens_ceiling: DEFAULT_MAX_TOKENS_CEILING,
            check_budget_before_send: false,
            per_request_reserve: 0,
        }
    }
}

/// 1 request で使う token の概算（正確な数ではない）: JSON にした request の byte 数 / 3（切り上げ）+
/// max_tokens。英文では多め、byte あたりの token が多い文字列では少なめになりうる
pub fn estimate_request_tokens(req: &MessageRequest) -> u64 {
    let bytes = serde_json::to_vec(req).map(|b| b.len()).unwrap_or(0) as u64;
    bytes.div_ceil(3).saturating_add(u64::from(req.max_tokens))
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
    /// LLM 呼び出しのエラー（中身は [`RunOutcome::error`]）
    LlmError,
    /// 未知の stop_reason など
    Other(String),
}

/// ループの結果
#[derive(Debug)]
pub struct RunOutcome {
    pub stop: RunStop,
    /// 履歴の最後の assistant message の本文（pause_turn の続きを合わせたもの）
    pub text: String,
    /// [`RunStop::LlmError`]（や deadline 中の送信失敗）の時のエラー
    pub error: Option<LlmError>,
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
    observer: Option<Arc<dyn RunObserver>>,
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
            observer: None,
        }
    }

    pub fn with_limits(mut self, limits: RunnerLimits) -> Self {
        self.limits = limits;
        self
    }

    /// 往復ごとに `observer` を呼ぶ（応答を受け取った直後、tool を実行する前）
    pub fn with_observer(mut self, observer: Arc<dyn RunObserver>) -> Self {
        self.observer = Some(observer);
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
            if self.limits.check_budget_before_send
                && let Some(b) = &self.limits.token_budget
                && b.remaining()
                    < estimate_request_tokens(&req).saturating_add(self.limits.per_request_reserve)
            {
                return Ok(state.finish(RunStop::TokenBudget));
            }
            let resp = match self
                .client
                .create_message_until(&req, self.limits.deadline)
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    // 課金された可能性がある分は 1 タスクの予算にも数える
                    if let Some(b) = &self.limits.token_budget {
                        b.charge(e.billed_estimate());
                    }
                    let stop = if matches!(e, LlmError::DeadlineExceeded) || self.past_deadline() {
                        RunStop::Deadline
                    } else {
                        RunStop::LlmError
                    };
                    return Ok(state.fail(stop, e));
                }
            };
            state.iterations += 1;
            state.add_usage(&resp.usage);
            if let Some(b) = &self.limits.token_budget {
                b.charge(resp.usage.total());
            }
            if let Some(o) = &self.observer
                && self
                    .with_deadline(o.on_iteration(state.iterations, &resp))
                    .await
                    .is_none()
            {
                state.last = Some(resp);
                return Ok(state.finish(RunStop::Deadline));
            }

            let tool_uses = match resp.tool_uses() {
                Ok(t) => t,
                Err(e) => {
                    state.last = Some(resp);
                    return Ok(state.fail(RunStop::LlmError, e));
                }
            };
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
        self.into_outcome(stop, None)
    }

    fn fail(self, stop: RunStop, error: LlmError) -> RunOutcome {
        self.into_outcome(stop, Some(error))
    }

    fn into_outcome(self, stop: RunStop, error: Option<LlmError>) -> RunOutcome {
        let text = self
            .messages
            .iter()
            .rev()
            .find(|m| m.role == Role::Assistant)
            .map(|m| text_of(&m.content))
            .unwrap_or_default();
        RunOutcome {
            stop,
            text,
            error,
            messages: self.messages,
            iterations: self.iterations,
            usage: self.usage,
            last_response: self.last,
        }
    }
}

/// content block の text をつなげる（[`MessageResponse::text`] と同じ規則）
fn text_of(content: &[Value]) -> String {
    content
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .collect()
}

/// tool 入力の簡易検証（object・required・1 段目の type / enum・additionalProperties:false）
pub(crate) fn validate_input(def: &ToolDef, input: &Value) -> std::result::Result<(), String> {
    let ToolDef::Custom { input_schema, .. } = def else {
        if !input.is_object() {
            return Err("input must be a JSON object".into());
        }
        return Ok(());
    };
    validate_against_schema(input_schema, input)
}

/// JSON schema の簡易検証（object・required・1 段目の type / enum・array の items の type・
/// additionalProperties:false）。MCP の typed tool の引数も server 側でこれで確かめる
pub(crate) fn validate_against_schema(
    input_schema: &Value,
    input: &Value,
) -> std::result::Result<(), String> {
    let Some(obj) = input.as_object() else {
        return Err("input must be a JSON object".into());
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
        if let (Some(items), Some(Value::String(item_ty))) = (
            value.as_array(),
            prop.get("items").and_then(|i| i.get("type")),
        ) && !items.iter().all(|x| type_matches(item_ty, x))
        {
            return Err(format!("items of `{key}` must be of type {item_ty}"));
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
