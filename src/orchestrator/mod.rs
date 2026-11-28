//! Orchestrator 実装

pub mod planner;
pub mod result_aggregator;
pub mod task_parser;

pub use planner::Planner;
pub use result_aggregator::{AggregatedMetrics, AggregatedResult, ResultAggregator};
pub use task_parser::{TaskInput, TaskParser};

use crate::core::subagent::{Subagent, SubagentEvent};
use crate::error::{Error, Result};
use crate::protocol::A2AProtocol;
use crate::types::{
    A2AMessage, ExecutionPattern, MessageType, OrchestrationPlan, Task, TaskMetadata, TaskResult,
    TaskResultPayload, TaskStatus,
};
use crate::utils::logger::{LogContext, LogFields, Logger};
use regex::Regex;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Instant;
use tokio::sync::broadcast;

static LENGTH_COND_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"previous\.results\.length\s*([><=]+)\s*(\d+)").unwrap());

pub struct Orchestrator {
    /// 登録順を保つ（TS の Map と同じく、capability での選択は先に登録したものを優先する）
    subagents: Vec<(String, Arc<dyn Subagent>)>,
    task_parser: TaskParser,
    planner: Planner,
    result_aggregator: ResultAggregator,
    logger: Logger,
    active_tasks: Mutex<HashMap<String, Task>>,
    events: broadcast::Sender<SubagentEvent>,
}

impl Default for Orchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl Orchestrator {
    pub fn new() -> Self {
        Orchestrator {
            subagents: Vec::new(),
            task_parser: TaskParser::new(),
            planner: Planner::new(),
            result_aggregator: ResultAggregator::new(),
            logger: Logger::new("orchestrator"),
            active_tasks: Mutex::new(HashMap::new()),
            events: broadcast::channel(1024).0,
        }
    }

    /// Subagent を登録（そのイベントは [`Orchestrator::subscribe`] に転送される）
    pub fn register_subagent(&mut self, subagent: Arc<dyn Subagent>) {
        let config = subagent.get_config();
        self.logger.info(
            &format!("Subagent registered: {}", config.name),
            Some(&json!({ "id": config.id })),
        );
        subagent.base().add_listener(self.events.clone());
        // 同じ id の再登録は位置を保ったまま置き換える（Map.set と同じ）
        match self.subagents.iter_mut().find(|(id, _)| *id == config.id) {
            Some(slot) => slot.1 = subagent,
            None => self.subagents.push((config.id.clone(), subagent)),
        }
    }

    /// 登録済み Subagent のイベント（progress / status_changed）を購読
    pub fn subscribe(&self) -> broadcast::Receiver<SubagentEvent> {
        self.events.subscribe()
    }

    pub fn get_subagent(&self, id: &str) -> Option<Arc<dyn Subagent>> {
        self.subagents
            .iter()
            .find(|(k, _)| k == id)
            .map(|(_, a)| a.clone())
    }

    /// 実行中のタスク ID
    pub fn active_task_ids(&self) -> Vec<String> {
        self.active_tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect()
    }

    /// タスクを実行
    pub async fn execute(
        &self,
        input: impl Into<TaskInput>,
        pattern: Option<ExecutionPattern>,
    ) -> Result<AggregatedResult> {
        let outcome: Result<AggregatedResult> = async {
            // 1. 解析
            let tasks = self.task_parser.parse(input.into());
            self.logger
                .info(&format!("Parsed {} task(s)", tasks.len()), None);
            // 2. 計画
            let plan = self.planner.create_plan(tasks, pattern);
            self.logger.info(
                &format!("Created plan with pattern: {}", pattern_name(plan.pattern)),
                None,
            );
            // 3. 実行
            let pattern = plan.pattern;
            let dependencies = plan.dependencies.clone();
            let results = self.execute_plan(plan).await?;
            // 4. 集約（hierarchical は dependencies で親子を分ける）
            let aggregated =
                self.result_aggregator
                    .aggregate(results, pattern, dependencies.as_deref())?;
            self.logger.info(
                "Execution completed",
                Some(&json!({ "success": aggregated.success, "metrics": aggregated.metrics })),
            );
            Ok(aggregated)
        }
        .await;
        if let Err(e) = &outcome {
            self.logger
                .error("Execution failed", Some(&json!({ "error": e.to_string() })));
        }
        outcome
    }

    async fn execute_plan(&self, plan: OrchestrationPlan) -> Result<Vec<TaskResult>> {
        match plan.pattern {
            ExecutionPattern::Sequential => self.execute_sequential(&plan).await,
            ExecutionPattern::Parallel => self.execute_parallel(plan.tasks).await,
            ExecutionPattern::Hierarchical => self.execute_hierarchical(&plan).await,
            ExecutionPattern::Conditional => self.execute_conditional(plan.tasks).await,
        }
    }

    /// 逐次実行。依存タスクの結果を `context.previous_results` に入れる。
    async fn execute_sequential(&self, plan: &OrchestrationPlan) -> Result<Vec<TaskResult>> {
        let mut results: Vec<TaskResult> = vec![];
        for task in &plan.tasks {
            let mut task = task.clone();
            if let Some(deps) = plan.dependencies_of(&task.id) {
                let dep_results: Vec<Value> = deps
                    .iter()
                    .filter_map(|d| results.iter().find(|r| &r.task_id == d))
                    .map(|r| serde_json::to_value(r).unwrap_or(Value::Null))
                    .collect();
                task.context
                    .get_or_insert_with(Default::default)
                    .extra
                    .insert("previous_results".into(), Value::Array(dep_results));
            }
            results.push(self.execute_task(task).await?);
        }
        Ok(results)
    }

    /// 並列実行。全タスクを最後まで走らせてから、失敗があれば最初のエラーを返す
    /// （Promise.all と同様に他のタスクは中断されない）。
    async fn execute_parallel(&self, tasks: Vec<Task>) -> Result<Vec<TaskResult>> {
        futures::future::join_all(tasks.into_iter().map(|t| self.execute_task(t)))
            .await
            .into_iter()
            .collect()
    }

    /// 階層実行。依存を持たない最初のタスクを親として先に実行し、残りを並列実行する。
    async fn execute_hierarchical(&self, plan: &OrchestrationPlan) -> Result<Vec<TaskResult>> {
        let Some(parent) = plan
            .tasks
            .iter()
            .find(|t| plan.dependencies_of(&t.id).is_none())
            .cloned()
        else {
            return self.execute_parallel(plan.tasks.clone()).await;
        };
        let parent_id = parent.id.clone();
        let parent_result = self.execute_task(parent).await?;
        let children: Vec<Task> = plan
            .tasks
            .iter()
            .filter(|t| t.id != parent_id)
            .cloned()
            .map(|mut t| {
                if let Some(v) = &parent_result.result {
                    t.context
                        .get_or_insert_with(Default::default)
                        .extra
                        .insert("parent_result".into(), v.clone());
                }
                t
            })
            .collect();
        let mut results = vec![parent_result];
        if !children.is_empty() {
            results.extend(self.execute_parallel(children).await?);
        }
        Ok(results)
    }

    /// 条件付き実行。`parameters.condition` が偽のタスクはスキップする。
    async fn execute_conditional(&self, tasks: Vec<Task>) -> Result<Vec<TaskResult>> {
        let mut results = vec![];
        for task in tasks {
            if task.param_truthy("condition")
                && !evaluate_condition(&task.parameters["condition"], &results)
            {
                continue;
            }
            results.push(self.execute_task(task).await?);
        }
        Ok(results)
    }

    /// 単一タスクを実行
    ///
    /// TS #44 と同じく task_id / correlation_id を固定した子ロガーで
    /// started / finished / failed を出す（#61）。`self.logger` は文脈を持たないので、
    /// `child` が `None` を親から引き継いでも、correlation_id の無いタスクに
    /// 別タスクの値が載ることはない（TS の `undefined` で上書きするのと同じ結果）。
    async fn execute_task(&self, task: Task) -> Result<TaskResult> {
        let id = task.id.clone();
        let log = self.logger.child(LogContext::task(
            id.clone(),
            task.correlation_id().map(str::to_string),
        ));
        let started_at = Instant::now();
        log.info_with(
            &format!("Executing task: {id}"),
            Some(&json!({ "action": task.action })),
            &LogFields::status("started"),
        );
        self.active_tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.clone(), task.clone());
        let result = match self.select_subagent(&task) {
            Ok(agent) => agent.execute(task).await,
            Err(e) => Err(e),
        };
        self.active_tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
        let duration = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
        match &result {
            Ok(r) => log.info_with(
                &format!("Task finished: {id}"),
                None,
                &LogFields::new(status_name(r.status), duration),
            ),
            Err(e) => log.error_with(
                &format!("Task failed: {id}"),
                Some(&json!({ "error": e.to_string() })),
                &LogFields::new("failed", duration),
            ),
        }
        result
    }

    /// action を capability に変換し、その capability を持つ最初の Subagent を選ぶ（TS #37）。
    /// 未知の action は capability 名そのものとして扱う。
    fn select_subagent(&self, task: &Task) -> Result<Arc<dyn Subagent>> {
        let capability = action_to_capability(&task.action);
        self.subagents
            .iter()
            .map(|(_, a)| a)
            .find(|a| {
                a.get_config()
                    .role
                    .capabilities
                    .iter()
                    .any(|c| c == capability)
            })
            .cloned()
            .ok_or_else(|| Error::NoCapableSubagent {
                capability: capability.to_string(),
                action: task.action.clone(),
            })
    }

    /// A2A メッセージを処理。heartbeat などレスポンス不要のときは None。
    pub async fn handle_a2a_message(&self, message: &A2AMessage) -> Option<A2AMessage> {
        if let Err(e) = A2AProtocol::validate_message(message) {
            self.logger
                .error("Invalid A2A message", Some(&json!({ "error": e })));
            return Some(A2AProtocol::create_error_message(
                "orchestrator",
                &message.from,
                &message.message_id,
                "INVALID_MESSAGE",
                &e,
                None,
            ));
        }

        match message.message_type? {
            MessageType::TaskRequest => {
                let Some(payload) = message.payload.as_ref().and_then(Value::as_object) else {
                    return Some(A2AProtocol::create_error_message(
                        "orchestrator",
                        &message.from,
                        &message.message_id,
                        "INVALID_MESSAGE",
                        "Missing payload for task_request",
                        None,
                    ));
                };
                let str_field = |k: &str| {
                    payload
                        .get(k)
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string()
                };
                let task = Task {
                    id: str_field("task_id"),
                    action: str_field("action"),
                    parameters: payload
                        .get("parameters")
                        .and_then(Value::as_object)
                        .cloned()
                        .unwrap_or_default(),
                    context: payload
                        .get("context")
                        .and_then(|c| serde_json::from_value(c.clone()).ok()),
                    metadata: Some(TaskMetadata {
                        correlation_id: message
                            .metadata
                            .as_ref()
                            .and_then(|m| m.correlation_id.clone()),
                        priority: message.metadata.as_ref().and_then(|m| m.priority),
                        ..Default::default()
                    }),
                };
                let task_id = task.id.clone();
                Some(match self.execute_task(task).await {
                    Ok(r) => A2AProtocol::create_task_result(
                        "orchestrator",
                        &message.from,
                        &message.message_id,
                        &TaskResultPayload {
                            task_id,
                            result: r.result,
                            error: r.error,
                        },
                        r.status,
                        r.metrics,
                    ),
                    Err(e) => A2AProtocol::create_error_message(
                        "orchestrator",
                        &message.from,
                        &message.message_id,
                        "EXECUTION_ERROR",
                        &e.to_string(),
                        None,
                    ),
                })
            }
            // heartbeat にはレスポンス不要
            _ => None,
        }
    }
}

/// action 名から必要な capability へ（TS の `ACTION_TO_CAPABILITY`）。
/// TS はオブジェクトを引くので `constructor` などプロトタイプ上の名前で関数が返る癖があるが、
/// Rust は意図して合わせず、未知の action はすべて capability 名そのものとして扱う。
fn action_to_capability(action: &str) -> &str {
    match action {
        "research" => "web_search",
        "codegen" => "code_generation",
        "review" => "code_analysis",
        "document" => "document_creation",
        other => other,
    }
}

/// TaskStatus の JSON 表現（serde の snake_case と同じ）
fn status_name(s: TaskStatus) -> &'static str {
    match s {
        TaskStatus::Pending => "pending",
        TaskStatus::Running => "running",
        TaskStatus::Completed => "completed",
        TaskStatus::Failed => "failed",
        TaskStatus::Cancelled => "cancelled",
    }
}

fn pattern_name(p: ExecutionPattern) -> &'static str {
    match p {
        ExecutionPattern::Sequential => "sequential",
        ExecutionPattern::Parallel => "parallel",
        ExecutionPattern::Hierarchical => "hierarchical",
        ExecutionPattern::Conditional => "conditional",
    }
}

/// 条件を評価する（簡易評価。評価できない条件は「実行する」）
pub fn evaluate_condition(condition: &Value, previous: &[TaskResult]) -> bool {
    let Some(condition) = condition.as_str() else {
        // TS では文字列以外は例外 → catch で true
        return true;
    };
    let success = previous.iter().all(|r| r.status == TaskStatus::Completed);
    if condition.contains("previous.success") {
        return success;
    }
    if condition.contains("previous.results.length")
        && let Some(c) = LENGTH_COND_RE.captures(condition)
    {
        let len = previous.len() as f64;
        let value: f64 = c[2].parse().unwrap_or(f64::INFINITY);
        return match &c[1] {
            ">" => len > value,
            "<" => len < value,
            ">=" => len >= value,
            "<=" => len <= value,
            "===" | "==" => len == value,
            _ => true,
        };
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn res(status: TaskStatus) -> TaskResult {
        TaskResult {
            task_id: "t".into(),
            status,
            result: None,
            error: None,
            confidence: None,
            metrics: None,
        }
    }

    #[test]
    fn conditions() {
        let ok = vec![res(TaskStatus::Completed), res(TaskStatus::Completed)];
        let bad = vec![res(TaskStatus::Completed), res(TaskStatus::Failed)];
        assert!(evaluate_condition(&json!("previous.success"), &ok));
        assert!(!evaluate_condition(&json!("previous.success"), &bad));
        assert!(evaluate_condition(&json!("previous.success"), &[]));
        assert!(evaluate_condition(
            &json!("previous.results.length > 1"),
            &ok
        ));
        assert!(!evaluate_condition(
            &json!("previous.results.length >= 3"),
            &ok
        ));
        assert!(evaluate_condition(
            &json!("previous.results.length === 2"),
            &ok
        ));
        assert!(evaluate_condition(
            &json!("previous.results.length =< 0"),
            &ok
        ));
        assert!(evaluate_condition(&json!("anything else"), &bad));
        assert!(evaluate_condition(&json!(true), &bad));
    }

    /// TS #44 の `executeTask` のログ（started / finished / failed）の移植のテスト（#61）
    mod task_logging {
        use super::*;
        use crate::core::subagent::AgentBase;
        use crate::types::{BehaviorConfig, InterfaceConfig, RoleConfig, SubagentConfig};
        use crate::utils::logger::{LogLevel, LogSink};
        use async_trait::async_trait;
        use std::time::Duration;

        /// capability `mock` を持つ Subagent。`parameters` で返り方を変える:
        /// `fail: true` → Err、`status: "failed"` → Ok(Failed)、それ以外 → Ok(Completed)。
        /// `sleep_ms` だけ待ってから返す。
        struct Mock {
            base: AgentBase,
        }

        impl Mock {
            fn new() -> Self {
                Mock {
                    base: AgentBase::new(
                        SubagentConfig {
                            id: "mock".into(),
                            name: "mock".into(),
                            version: "0".into(),
                            role: RoleConfig {
                                description: String::new(),
                                capabilities: vec!["mock".into()],
                                constraints: None,
                            },
                            interface: InterfaceConfig {
                                input: vec![],
                                output: vec![],
                            },
                            tools: vec![],
                            behavior: BehaviorConfig::default(),
                        },
                        vec![],
                    ),
                }
            }
        }

        #[async_trait]
        impl Subagent for Mock {
            fn base(&self) -> &AgentBase {
                &self.base
            }

            async fn execute(&self, task: Task) -> Result<TaskResult> {
                if let Some(ms) = task.parameters.get("sleep_ms").and_then(Value::as_u64) {
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                }
                if task.param_truthy("fail") {
                    return Err(Error::InvalidParameter("mock boom".into()));
                }
                let status =
                    if task.parameters.get("status").and_then(Value::as_str) == Some("failed") {
                        TaskStatus::Failed
                    } else {
                        TaskStatus::Completed
                    };
                Ok(TaskResult {
                    task_id: task.id.clone(),
                    ..res(status)
                })
            }
        }

        #[test]
        fn status_name_matches_serde() {
            for s in [
                TaskStatus::Pending,
                TaskStatus::Running,
                TaskStatus::Completed,
                TaskStatus::Failed,
                TaskStatus::Cancelled,
            ] {
                assert_eq!(serde_json::to_value(s).unwrap(), status_name(s));
            }
        }

        fn orchestrator() -> (Orchestrator, LogSink) {
            let mut o = Orchestrator::new();
            o.register_subagent(Arc::new(Mock::new()));
            let (logger, sink) = Logger::capturing(Some("orchestrator".into()), LogLevel::Debug);
            o.logger = logger;
            (o, sink)
        }

        fn task(id: &str, action: &str, params: Value, cid: Option<&str>) -> Task {
            let mut t = Task::new(id, action, params);
            t.metadata = cid.map(|c| TaskMetadata {
                correlation_id: Some(c.into()),
                ..Default::default()
            });
            t
        }

        /// message が一致するログを 1 件だけ取り出す
        fn entry(sink: &LogSink, message: &str) -> Value {
            let es: Vec<Value> = sink
                .lock()
                .unwrap()
                .iter()
                .map(|s| serde_json::from_str::<Value>(s).unwrap())
                .filter(|e| e["message"] == message)
                .collect();
            assert_eq!(es.len(), 1, "{message}: {es:?}");
            es.into_iter().next().unwrap()
        }

        #[tokio::test]
        async fn started_and_finished_carry_task_context_status_and_duration() {
            let (o, sink) = orchestrator();
            let r = o
                .execute_task(task("t1", "mock", json!({ "sleep_ms": 20 }), Some("c1")))
                .await
                .unwrap();
            assert_eq!(r.status, TaskStatus::Completed);

            let s = entry(&sink, "Executing task: t1");
            assert_eq!(s["level"], "INFO");
            assert_eq!(s["agent_id"], "orchestrator");
            assert_eq!(s["task_id"], "t1");
            assert_eq!(s["correlation_id"], "c1");
            assert_eq!(s["status"], "started");
            assert_eq!(s["data"], json!({ "action": "mock" }));
            assert!(s.get("duration").is_none(), "{s}");

            let f = entry(&sink, "Task finished: t1");
            assert_eq!(f["level"], "INFO");
            assert_eq!(f["task_id"], "t1");
            assert_eq!(f["correlation_id"], "c1");
            assert_eq!(f["status"], "completed");
            assert!(f["duration"].as_u64().unwrap() >= 20, "{f}");
            assert!(f.get("data").is_none(), "{f}");
        }

        #[tokio::test]
        async fn finished_uses_result_status_even_when_failed() {
            let (o, sink) = orchestrator();
            let r = o
                .execute_task(task("t2", "mock", json!({ "status": "failed" }), None))
                .await
                .unwrap();
            assert_eq!(r.status, TaskStatus::Failed);
            let f = entry(&sink, "Task finished: t2");
            assert_eq!(f["level"], "INFO");
            assert_eq!(f["status"], "failed");
            assert!(f["duration"].is_u64(), "{f}");
        }

        #[tokio::test]
        async fn agent_error_logs_task_failed() {
            let (o, sink) = orchestrator();
            let e = o
                .execute_task(task(
                    "t3",
                    "mock",
                    json!({ "fail": true, "sleep_ms": 20 }),
                    Some("c3"),
                ))
                .await
                .unwrap_err();
            let f = entry(&sink, "Task failed: t3");
            assert_eq!(f["level"], "ERROR");
            assert_eq!(f["task_id"], "t3");
            assert_eq!(f["correlation_id"], "c3");
            assert_eq!(f["status"], "failed");
            assert!(f["duration"].as_u64().unwrap() >= 20, "{f}");
            assert_eq!(f["data"], json!({ "error": e.to_string() }));
            assert!(
                !sink
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|s| s.contains("Task finished")),
                "finished must not be logged on error"
            );
            assert!(o.active_task_ids().is_empty());
        }

        #[tokio::test]
        async fn no_capable_subagent_logs_task_failed() {
            let (o, sink) = orchestrator();
            let e = o
                .execute_task(task("t4", "unknown_action", json!({}), None))
                .await
                .unwrap_err();
            entry(&sink, "Executing task: t4");
            let f = entry(&sink, "Task failed: t4");
            assert_eq!(f["status"], "failed");
            assert!(f["duration"].is_u64(), "{f}");
            assert_eq!(f["data"]["error"], e.to_string());
        }

        #[tokio::test]
        async fn task_without_correlation_id_does_not_inherit_another() {
            let (o, sink) = orchestrator();
            o.execute(
                vec![
                    task("a", "mock", json!({}), Some("ca")),
                    task("b", "mock", json!({}), None),
                ],
                Some(ExecutionPattern::Parallel),
            )
            .await
            .unwrap();
            for m in ["Executing task: a", "Task finished: a"] {
                assert_eq!(entry(&sink, m)["correlation_id"], "ca", "{m}");
            }
            for m in ["Executing task: b", "Task finished: b"] {
                let e = entry(&sink, m);
                assert_eq!(e["task_id"], "b");
                assert!(e.get("correlation_id").is_none(), "{m}: {e}");
            }
            // 親ロガーには task の文脈が残らない
            assert_eq!(o.logger.context(), &LogContext::default());
            let parsed = entry(&sink, "Parsed 2 task(s)");
            assert!(parsed.get("task_id").is_none(), "{parsed}");
        }
    }
}
