//! Subagentシステムの型定義

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;

/// JSON オブジェクト（TS の `Record<string, any>` 相当）
pub type JsonMap = Map<String, Value>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Idle,
    Running,
    Waiting,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageType {
    TaskRequest,
    TaskResult,
    Progress,
    Error,
    Heartbeat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionPattern {
    Sequential,
    Parallel,
    Hierarchical,
    Conditional,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Task {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub parameters: JsonMap,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<TaskContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<TaskMetadata>,
}

impl Task {
    pub fn new(id: impl Into<String>, action: impl Into<String>, parameters: Value) -> Self {
        Task {
            id: id.into(),
            action: action.into(),
            parameters: match parameters {
                Value::Object(m) => m,
                _ => JsonMap::new(),
            },
            context: None,
            metadata: None,
        }
    }

    /// `context.parent_task`
    pub fn parent_task(&self) -> Option<&str> {
        self.context.as_ref()?.parent_task.as_deref()
    }

    /// `metadata.correlation_id`
    pub fn correlation_id(&self) -> Option<&str> {
        self.metadata.as_ref()?.correlation_id.as_deref()
    }

    /// 文字列パラメータを取得
    pub fn param_str(&self, key: &str) -> Option<&str> {
        self.parameters.get(key).and_then(Value::as_str)
    }

    /// JS の truthy 判定でパラメータの有無を確認
    pub fn param_truthy(&self, key: &str) -> bool {
        self.parameters.get(key).is_some_and(is_truthy)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TaskContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_task: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_context: Option<SharedContext>,
    #[serde(flatten)]
    pub extra: JsonMap,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TaskMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<Priority>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(flatten)]
    pub extra: JsonMap,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskResult {
    #[serde(rename = "taskId")]
    pub task_id: String,
    pub status: TaskStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorInfo>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "ser_js_opt_f64"
    )]
    pub confidence: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<TaskMetrics>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorInfo {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TaskMetrics {
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_used: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools_called: Option<u64>,
    #[serde(flatten)]
    pub extra: JsonMap,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SharedContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(flatten)]
    pub extra: JsonMap,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SubagentContextState {
    pub memory: HashMap<String, Value>,
    pub history: Vec<TaskResult>,
    #[serde(rename = "sharedContext", skip_serializing_if = "Option::is_none")]
    pub shared_context: Option<SharedContext>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SubagentMetrics {
    #[serde(rename = "tasksCompleted")]
    pub tasks_completed: u64,
    #[serde(rename = "avgDuration", serialize_with = "ser_js_f64")]
    pub avg_duration: f64,
    #[serde(rename = "successRate", serialize_with = "ser_js_f64")]
    pub success_rate: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SubagentState {
    pub id: String,
    pub status: AgentStatus,
    #[serde(rename = "currentTask")]
    pub current_task: Option<Task>,
    pub context: SubagentContextState,
    pub metrics: SubagentMetrics,
}

/// A2A メッセージ。TS の `A2AMessage` と `A2AResponse` を1つの型で表す
/// （レスポンス専用フィールドは Option）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct A2AMessage {
    #[serde(default)]
    pub message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<String>,
    #[serde(default)]
    pub timestamp: String,
    #[serde(default)]
    pub from: String,
    #[serde(default)]
    pub to: String,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub message_type: Option<MessageType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<TaskStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<MessageMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<TaskMetrics>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MessageMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<Priority>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(flatten)]
    pub extra: JsonMap,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskRequestPayload {
    pub task_id: String,
    pub action: String,
    #[serde(default)]
    pub parameters: JsonMap,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<TaskContext>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskResultPayload {
    pub task_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubagentConfig {
    pub id: String,
    pub name: String,
    pub version: String,
    pub role: RoleConfig,
    pub interface: InterfaceConfig,
    pub tools: Vec<String>,
    pub behavior: BehaviorConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoleConfig {
    pub description: String,
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constraints: Option<JsonMap>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InterfaceConfig {
    pub input: Vec<ParameterDefinition>,
    pub output: Vec<ParameterDefinition>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParameterDefinition {
    pub name: String,
    #[serde(rename = "type")]
    pub param_type: String,
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<Value>,
}

impl ParameterDefinition {
    pub fn new(name: &str, param_type: &str, required: bool) -> Self {
        ParameterDefinition {
            name: name.into(),
            param_type: param_type.into(),
            required,
            schema: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BehaviorConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_policy: Option<RetryPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backoff {
    Exponential,
    Linear,
    Fixed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub backoff: Backoff,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_delay_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ErrorHandlingStrategy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<ErrorAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation_error: Option<ErrorAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_failure: Option<ErrorAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partial_failure: Option<ErrorAction>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ErrorAction {
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alternatives: Option<Vec<ToolAlternative>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub escalate_after: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify: Option<bool>,
}

impl ErrorAction {
    pub fn new(action: &str) -> Self {
        ErrorAction {
            action: action.into(),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolAlternative {
    pub primary: String,
    pub backup: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OrchestrationPlan {
    pub pattern: ExecutionPattern,
    pub tasks: Vec<Task>,
    /// タスクID → 依存タスクID（挿入順を保持）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<Vec<(String, Vec<String>)>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conditions: Option<Vec<ConditionalRule>>,
}

impl OrchestrationPlan {
    pub fn dependencies_of(&self, task_id: &str) -> Option<&[String]> {
        self.dependencies
            .as_ref()?
            .iter()
            .find(|(id, _)| id == task_id)
            .map(|(_, deps)| deps.as_slice())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConditionalRule {
    pub condition: String,
    pub target_subagent: String,
    pub action: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgressReport {
    #[serde(rename = "taskId")]
    pub task_id: String,
    #[serde(serialize_with = "ser_js_f64")]
    pub progress: f64,
    pub message: String,
    pub timestamp: String,
}

/// JavaScript の truthy 判定
pub fn is_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// 現在時刻を ISO 8601（ミリ秒、UTC `Z`）で返す。JS の `toISOString()` と同形式。
pub fn now_iso() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

/// JS の `string.length`（UTF-16 コード単位数）
pub fn js_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// JS の `s.substring(0, n)`（UTF-16 単位で切り詰め。サロゲートペアの途中で切れた場合は U+FFFD）
pub fn js_substring(s: &str, n: usize) -> String {
    let units: Vec<u16> = s.encode_utf16().take(n).collect();
    String::from_utf16_lossy(&units)
}

/// f64 を JS と同じ表記の JSON 数値にする（整数値なら `5`、そうでなければ `0.5`）
pub fn js_number(f: f64) -> Value {
    if f.is_finite() && f.fract() == 0.0 && f.abs() < 9_007_199_254_740_992.0 {
        Value::from(f as i64)
    } else {
        serde_json::Number::from_f64(f)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}

/// `Option<f64>` を JS 表記でシリアライズする（`#[serde(serialize_with)]` 用）
pub fn ser_js_opt_f64<S: serde::Serializer>(v: &Option<f64>, s: S) -> Result<S::Ok, S::Error> {
    match v {
        Some(f) => js_number(*f).serialize(s),
        None => s.serialize_none(),
    }
}

/// `f64` を JS 表記でシリアライズする（`#[serde(serialize_with)]` 用）
pub fn ser_js_f64<S: serde::Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
    js_number(*v).serialize(s)
}
