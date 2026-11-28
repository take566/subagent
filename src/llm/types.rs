//! Claude Messages API（`POST /v1/messages`、`anthropic-version: 2026-10-21`）の request / response 型
//!
//! - request は送ってよい field だけを持つ。`temperature` / `top_p` / `top_k` は 2026-10-21 で
//!   400 になり、`tool_choice` の `any` / `tool` は claude-opus-5-5 で 400 になるので field 自体を持たない。
//! - 応答の assistant content は raw の [`Value`] のまま持ち、次の request にそのまま送り返す
//!   （thinking の signature、server_tool_use、web_search の結果、citations、未知の block を壊さない）。

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value, json};

use super::client::LlmError;

/// 既定のモデル
pub const DEFAULT_MODEL: &str = "claude-opus-5-5";

/// text editor tool の type（schema はモデル側に組み込み）
pub const TEXT_EDITOR_TYPE: &str = "text_editor_20250728";
/// text editor tool の name（type と組で固定）
pub const TEXT_EDITOR_NAME: &str = "str_replace_based_edit_tool";
/// web search server tool の type
pub const WEB_SEARCH_TYPE: &str = "web_search_20260209";
/// web fetch server tool の type
pub const WEB_FETCH_TYPE: &str = "web_fetch_20260209";

/// message の role
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

/// 1 つの message。content は content block の配列を raw JSON で持つ
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<Value>,
}

impl Message {
    /// text 1 個の user message
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![json!({"type": "text", "text": text.into()})],
        }
    }

    /// 応答の content をそのまま持つ assistant message
    pub fn assistant(content: Vec<Value>) -> Self {
        Self {
            role: Role::Assistant,
            content,
        }
    }

    /// tool_result block を全部まとめた 1 つの user message
    pub fn tool_results(blocks: Vec<Value>) -> Self {
        Self {
            role: Role::User,
            content: blocks,
        }
    }

    /// `tool_result` block。`is_error` は true の時だけ付ける
    pub fn tool_result_block(
        tool_use_id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) -> Value {
        let mut block = json!({
            "type": "tool_result",
            "tool_use_id": tool_use_id.into(),
            "content": content.into(),
        });
        if is_error {
            block["is_error"] = Value::Bool(true);
        }
        block
    }
}

/// thinking の設定。claude-opus-5-5 は adaptive のみ（disabled / budget_tokens は 400）
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ThinkingConfig {
    Adaptive {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<ThinkingDisplay>,
    },
}

impl Default for ThinkingConfig {
    fn default() -> Self {
        ThinkingConfig::Adaptive { display: None }
    }
}

/// thinking の表示
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingDisplay {
    Summarized,
}

/// `output_config.effort`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

/// `output_config`。`format` は structured output の指定を raw JSON でそのまま渡す
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct OutputConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<Effort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<Value>,
}

/// `cache_control`（top-level に付けると最後の cacheable block に自動で置かれる）
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CacheControl {
    #[serde(rename = "type")]
    pub kind: CacheType,
    /// `"1h"` など。None は既定（5 分）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl: Option<String>,
}

impl CacheControl {
    pub fn ephemeral() -> Self {
        Self {
            kind: CacheType::Ephemeral,
            ttl: None,
        }
    }
}

/// cache の種類
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheType {
    Ephemeral,
}

/// tool の定義
///
/// どの variant も `options` に追加の field（custom の `strict` / `cache_control` /
/// `eager_input_streaming`、text editor の `max_characters` など）を持ち、そのまま送る。
/// 中心の field（`type` / `name` / `description` / `input_schema`）は options で上書きできない。
#[derive(Debug, Clone, PartialEq)]
pub enum ToolDef {
    /// user-defined tool（name / description / input_schema）
    Custom {
        name: String,
        description: String,
        input_schema: Value,
        options: Map<String, Value>,
    },
    /// Anthropic-defined の text editor（client 実行、schema 無し）
    TextEditor { options: Map<String, Value> },
    /// server tool の web search
    WebSearch { options: Map<String, Value> },
    /// server tool の web fetch
    WebFetch { options: Map<String, Value> },
}

/// options で上書きさせない field
const RESERVED_TOOL_FIELDS: [&str; 4] = ["type", "name", "description", "input_schema"];

impl ToolDef {
    pub fn custom(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: Value,
    ) -> Self {
        ToolDef::Custom {
            name: name.into(),
            description: description.into(),
            input_schema,
            options: Map::new(),
        }
    }

    pub fn text_editor() -> Self {
        ToolDef::TextEditor {
            options: Map::new(),
        }
    }

    pub fn web_search() -> Self {
        ToolDef::WebSearch {
            options: Map::new(),
        }
    }

    pub fn web_fetch() -> Self {
        ToolDef::WebFetch {
            options: Map::new(),
        }
    }

    /// 追加の field を 1 つ足す（中心の field の名前は送る時に無視される）
    pub fn with_option(mut self, key: impl Into<String>, value: Value) -> Self {
        let options = match &mut self {
            ToolDef::Custom { options, .. }
            | ToolDef::TextEditor { options }
            | ToolDef::WebSearch { options }
            | ToolDef::WebFetch { options } => options,
        };
        options.insert(key.into(), value);
        self
    }

    /// model が tool_use で返す name
    pub fn name(&self) -> &str {
        match self {
            ToolDef::Custom { name, .. } => name,
            ToolDef::TextEditor { .. } => TEXT_EDITOR_NAME,
            ToolDef::WebSearch { .. } => "web_search",
            ToolDef::WebFetch { .. } => "web_fetch",
        }
    }

    /// API が実行する server tool か（client は実行しない）
    pub fn is_server_tool(&self) -> bool {
        matches!(self, ToolDef::WebSearch { .. } | ToolDef::WebFetch { .. })
    }

    /// request に入れる JSON
    pub fn to_json(&self) -> Value {
        let (mut obj, options) = match self {
            ToolDef::Custom {
                name,
                description,
                input_schema,
                options,
            } => {
                let mut obj = Map::new();
                obj.insert("name".into(), Value::String(name.clone()));
                obj.insert("description".into(), Value::String(description.clone()));
                obj.insert("input_schema".into(), input_schema.clone());
                (obj, options)
            }
            ToolDef::TextEditor { options }
            | ToolDef::WebSearch { options }
            | ToolDef::WebFetch { options } => {
                let kind = match self {
                    ToolDef::TextEditor { .. } => TEXT_EDITOR_TYPE,
                    ToolDef::WebSearch { .. } => WEB_SEARCH_TYPE,
                    _ => WEB_FETCH_TYPE,
                };
                let mut obj = Map::new();
                obj.insert("type".into(), Value::String(kind.into()));
                obj.insert("name".into(), Value::String(self.name().into()));
                (obj, options)
            }
        };
        for (k, v) in options {
            if !RESERVED_TOOL_FIELDS.contains(&k.as_str()) {
                obj.insert(k.clone(), v.clone());
            }
        }
        Value::Object(obj)
    }
}

impl Serialize for ToolDef {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_json().serialize(serializer)
    }
}

/// `POST /v1/messages` の request body
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MessageRequest {
    pub model: String,
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_config: Option<OutputConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

impl MessageRequest {
    /// 既定値（claude-opus-5-5、thinking adaptive、top-level cache_control）の request
    pub fn new(max_tokens: u32, messages: Vec<Message>) -> Self {
        Self {
            model: DEFAULT_MODEL.to_string(),
            max_tokens,
            system: None,
            messages,
            tools: Vec::new(),
            thinking: Some(ThinkingConfig::default()),
            output_config: None,
            cache_control: Some(CacheControl::ephemeral()),
        }
    }
}

/// 応答の stop_reason。知らない値は [`StopReason::Other`]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    StopSequence,
    PauseTurn,
    Refusal,
    /// 未知の値（null / 欠落は空文字）
    Other(String),
}

impl StopReason {
    pub fn as_str(&self) -> &str {
        match self {
            StopReason::EndTurn => "end_turn",
            StopReason::ToolUse => "tool_use",
            StopReason::MaxTokens => "max_tokens",
            StopReason::StopSequence => "stop_sequence",
            StopReason::PauseTurn => "pause_turn",
            StopReason::Refusal => "refusal",
            StopReason::Other(s) => s,
        }
    }

    fn from_str_value(s: &str) -> Self {
        match s {
            "end_turn" => StopReason::EndTurn,
            "tool_use" => StopReason::ToolUse,
            "max_tokens" => StopReason::MaxTokens,
            "stop_sequence" => StopReason::StopSequence,
            "pause_turn" => StopReason::PauseTurn,
            "refusal" => StopReason::Refusal,
            other => StopReason::Other(other.to_string()),
        }
    }
}

impl Default for StopReason {
    fn default() -> Self {
        StopReason::Other(String::new())
    }
}

impl Serialize for StopReason {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for StopReason {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s: Option<String> = Option::deserialize(deserializer)?;
        Ok(s.map(|s| StopReason::from_str_value(&s))
            .unwrap_or_default())
    }
}

/// 応答の usage
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
}

impl Usage {
    /// 予算に数える token 数（入力・cache 書き込み・cache 読み出し・出力の合計）
    pub fn total(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cache_creation_input_tokens)
            .saturating_add(self.cache_read_input_tokens)
    }
}

fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// `POST /v1/messages` の応答
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageResponse {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub model: String,
    /// raw の content block。そのまま送り返す
    #[serde(default)]
    pub content: Vec<Value>,
    #[serde(default)]
    pub stop_reason: StopReason,
    #[serde(default, deserialize_with = "null_as_default")]
    pub stop_sequence: Option<String>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub usage: Usage,
}

/// client が実行する tool 呼び出し（`type: "tool_use"` の block）
#[derive(Debug, Clone, PartialEq)]
pub struct ToolUse {
    pub id: String,
    pub name: String,
    pub input: Value,
}

impl MessageResponse {
    /// content を verbatim に持つ assistant message
    pub fn to_assistant_message(&self) -> Message {
        Message::assistant(self.content.clone())
    }

    /// text block の本文をつなげたもの（citations は含めない）
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("")
    }

    /// client 側で実行する tool_use（server_tool_use は含めない）
    ///
    /// `id` か `name` が無い・空・文字列でない block は、空の id で tool_result を返すと
    /// 400 になるので [`LlmError::Decode`] にする。
    pub fn tool_uses(&self) -> Result<Vec<ToolUse>, LlmError> {
        self.content
            .iter()
            .enumerate()
            .filter(|(_, b)| b.get("type").and_then(Value::as_str) == Some("tool_use"))
            .map(|(i, b)| {
                let field = |k: &str| {
                    b.get(k)
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .ok_or_else(|| {
                            LlmError::Decode(format!(
                                "content[{i}]: tool_use block has no valid `{k}`"
                            ))
                        })
                };
                Ok(ToolUse {
                    id: field("id")?,
                    name: field("name")?,
                    input: b.get("input").cloned().unwrap_or(Value::Null),
                })
            })
            .collect()
    }
}
