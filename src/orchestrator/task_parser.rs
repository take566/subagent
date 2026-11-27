//! Task Parser 実装

use crate::types::{is_truthy, now_iso, JsonMap, Task, TaskContext, TaskMetadata};
use rand::RngExt;
use serde_json::{json, Value};

/// Orchestrator への入力（TS の `string | Task | Task[]`）
#[derive(Debug, Clone)]
pub enum TaskInput {
    /// JSON 文字列、または自由記述（`execute` タスクの query になる）
    Text(String),
    One(Box<Task>),
    Many(Vec<Task>),
}

impl From<&str> for TaskInput {
    fn from(s: &str) -> Self {
        TaskInput::Text(s.into())
    }
}
impl From<String> for TaskInput {
    fn from(s: String) -> Self {
        TaskInput::Text(s)
    }
}
impl From<Task> for TaskInput {
    fn from(t: Task) -> Self {
        TaskInput::One(Box::new(t))
    }
}
impl From<Vec<Task>> for TaskInput {
    fn from(t: Vec<Task>) -> Self {
        TaskInput::Many(t)
    }
}

#[derive(Debug, Default, Clone)]
pub struct TaskParser;

impl TaskParser {
    pub fn new() -> Self {
        TaskParser
    }

    /// タスクを解析して構造化
    pub fn parse(&self, input: TaskInput) -> Vec<Task> {
        match input {
            TaskInput::Many(tasks) => tasks.into_iter().map(normalize_task).collect(),
            TaskInput::One(task) => vec![normalize_task(*task)],
            TaskInput::Text(s) => self.parse_from_string(&s),
        }
    }

    /// 文字列からタスクを解析。JSON として解釈できなければ単純な `execute` タスクにする。
    fn parse_from_string(&self, input: &str) -> Vec<Task> {
        let parsed = serde_json::from_str::<Value>(input).ok().and_then(|v| match v {
            Value::Array(items) => items.into_iter().map(normalize_value).collect(),
            other => normalize_value(other).map(|t| vec![t]),
        });
        parsed.unwrap_or_else(|| {
            vec![Task {
                id: generate_task_id(),
                action: "execute".into(),
                parameters: json!({ "query": input }).as_object().cloned().unwrap_or_default(),
                context: None,
                metadata: None,
            }]
        })
    }

    /// タスクの依存関係を抽出（タスクID → 依存タスクID。依存のないタスクは含めない）
    pub fn extract_dependencies(&self, tasks: &[Task]) -> Vec<(String, Vec<String>)> {
        let mut out = vec![];
        for task in tasks {
            let mut deps = vec![];
            if let Some(p) = task.parent_task().filter(|p| !p.is_empty()) {
                deps.push(p.to_string());
            }
            if let Some(cid) = task.correlation_id().filter(|c| !c.is_empty()) {
                deps.extend(
                    tasks
                        .iter()
                        .filter(|t| t.correlation_id() == Some(cid) && t.id != task.id)
                        .map(|t| t.id.clone()),
                );
            }
            if !deps.is_empty() {
                out.push((task.id.clone(), deps));
            }
        }
        out
    }
}

/// 型付きタスクを正規化（空の id / action を補う。created_at を付与）
pub fn normalize_task(mut task: Task) -> Task {
    if task.id.is_empty() {
        task.id = generate_task_id();
    }
    if task.action.is_empty() {
        task.action = "execute".into();
    }
    let mut metadata = task.metadata.take().unwrap_or_default();
    if metadata.created_at.as_deref().unwrap_or("").is_empty() {
        metadata.created_at = Some(now_iso());
    }
    task.metadata = Some(metadata);
    task
}

/// 任意の JSON 値をタスクに正規化する。TS 版と同じく寛容に扱う:
/// オブジェクト以外のプリミティブは空タスク、`null` は解析失敗（None）。
fn normalize_value(v: Value) -> Option<Task> {
    let obj = match v {
        Value::Null => return None,
        Value::Object(m) => m,
        _ => JsonMap::new(),
    };
    let scalar = |key: &str| -> String {
        match obj.get(key) {
            Some(v) if is_truthy(v) => match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            },
            _ => String::new(),
        }
    };
    let parameters = match obj.get("parameters") {
        Some(Value::Object(m)) => m.clone(),
        _ => JsonMap::new(),
    };
    let context = match obj.get("context") {
        Some(Value::Object(m)) => Some(lenient_context(m)),
        _ => None,
    };
    let metadata = match obj.get("metadata") {
        Some(Value::Object(m)) => Some(lenient_metadata(m)),
        _ => None,
    };
    Some(normalize_task(Task {
        id: scalar("id"),
        action: scalar("action"),
        parameters,
        context,
        metadata,
    }))
}

/// 既知フィールドを1つずつ取り出す。型の合わないフィールドは extra にそのまま残し、
/// 他のフィールド（parent_task など）は失わない。
fn take_field<T: serde::de::DeserializeOwned>(extra: &mut JsonMap, key: &str) -> Option<T> {
    let v = extra.get(key)?;
    let parsed = serde_json::from_value::<T>(v.clone()).ok()?;
    extra.remove(key);
    Some(parsed)
}

fn lenient_context(m: &JsonMap) -> TaskContext {
    let mut extra = m.clone();
    TaskContext {
        parent_task: take_field(&mut extra, "parent_task"),
        deadline: take_field(&mut extra, "deadline"),
        shared_context: take_field(&mut extra, "shared_context"),
        extra,
    }
}

fn lenient_metadata(m: &JsonMap) -> TaskMetadata {
    let mut extra = m.clone();
    let metadata = TaskMetadata {
        priority: take_field(&mut extra, "priority"),
        correlation_id: take_field(&mut extra, "correlation_id"),
        retry_count: take_field(&mut extra, "retry_count"),
        created_at: take_field(&mut extra, "created_at"),
        extra,
    };
    // 型の合わない created_at は normalize_task で付け直すので、重複キーを避けて捨てる
    let mut metadata = metadata;
    metadata.extra.remove("created_at");
    metadata
}

/// タスク ID を生成（`task-<epoch ms>-<9桁 base36>`）
pub fn generate_task_id() -> String {
    const CHARS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut rng = rand::rng();
    let suffix: String = (0..9)
        .map(|_| CHARS[rng.random_range(0..CHARS.len())] as char)
        .collect();
    format!("task-{}-{suffix}", chrono::Utc::now().timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p() -> TaskParser {
        TaskParser::new()
    }

    #[test]
    fn plain_text_becomes_execute_task() {
        let t = &p().parse("hello world".into())[0];
        assert_eq!(t.action, "execute");
        assert_eq!(t.parameters["query"], "hello world");
        assert!(t.id.starts_with("task-"));
    }

    #[test]
    fn json_object_and_array() {
        let ts = p().parse(r#"[{"id":"a","action":"research","parameters":{"q":1}},{"action":"x"}]"#.into());
        assert_eq!(ts.len(), 2);
        assert_eq!(ts[0].id, "a");
        assert_eq!(ts[0].parameters["q"], 1);
        assert!(ts[0].metadata.as_ref().unwrap().created_at.is_some());
        assert!(ts[1].id.starts_with("task-"));
    }

    #[test]
    fn lenient_like_ts() {
        // 数値 id も受け付ける
        assert_eq!(p().parse(r#"{"id":123}"#.into())[0].id, "123");
        // プリミティブ JSON は空のタスク
        let t = &p().parse("42".into())[0];
        assert_eq!(t.action, "execute");
        assert!(t.parameters.is_empty());
        // null は TS で TypeError → catch されて文字列タスクになる
        assert_eq!(p().parse("null".into())[0].parameters["query"], "null");
        assert_eq!(p().parse("[1,null]".into())[0].parameters["query"], "[1,null]");
        // 型の合わない context でも落ちない
        let t = &p().parse(r#"{"context":{"parent_task":5}}"#.into())[0];
        assert_eq!(t.parent_task(), None);
        // 1つのフィールドの型が合わなくても、他のフィールドは失われない
        let t = &p().parse(r#"{"metadata":{"correlation_id":"c","priority":"urgent"},"context":{"parent_task":"a","deadline":1}}"#.into())[0];
        assert_eq!(t.correlation_id(), Some("c"));
        assert_eq!(t.parent_task(), Some("a"));
        let meta = t.metadata.as_ref().unwrap();
        assert_eq!(meta.extra["priority"], "urgent");
        // created_at の型が違っても JSON キーが重複しない
        let t = &p().parse(r#"{"metadata":{"created_at":5}}"#.into())[0];
        let s = serde_json::to_string(t).unwrap();
        assert_eq!(s.matches("created_at").count(), 1, "{s}");
    }

    #[test]
    fn typed_task_normalized() {
        let t = &p().parse(Task::default().into())[0];
        assert!(!t.id.is_empty());
        assert_eq!(t.action, "execute");
    }

    #[test]
    fn id_format() {
        let id = generate_task_id();
        let parts: Vec<_> = id.split('-').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[2].len(), 9);
    }

    #[test]
    fn dependencies() {
        let mut a = Task::new("a", "x", json!({}));
        let mut b = Task::new("b", "x", json!({}));
        b.context = Some(TaskContext { parent_task: Some("a".into()), ..Default::default() });
        a.metadata = Some(TaskMetadata { correlation_id: Some("c".into()), ..Default::default() });
        let mut c = Task::new("c", "x", json!({}));
        c.metadata = Some(TaskMetadata { correlation_id: Some("c".into()), ..Default::default() });
        let d = p().extract_dependencies(&[a, b, c]);
        assert_eq!(
            d,
            vec![
                ("a".to_string(), vec!["c".to_string()]),
                ("b".to_string(), vec!["a".to_string()]),
                ("c".to_string(), vec!["a".to_string()]),
            ]
        );
    }
}
