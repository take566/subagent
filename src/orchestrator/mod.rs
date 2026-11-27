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
    A2AMessage, ExecutionPattern, MessageType, OrchestrationPlan, Task, TaskMetadata,
    TaskResult, TaskResultPayload, TaskStatus,
};
use crate::utils::logger::Logger;
use regex::Regex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use tokio::sync::broadcast;

static LENGTH_COND_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"previous\.results\.length\s*([><=]+)\s*(\d+)").unwrap());

pub struct Orchestrator {
    subagents: HashMap<String, Arc<dyn Subagent>>,
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
            subagents: HashMap::new(),
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
        self.subagents.insert(config.id.clone(), subagent);
    }

    /// 登録済み Subagent のイベント（progress / status_changed）を購読
    pub fn subscribe(&self) -> broadcast::Receiver<SubagentEvent> {
        self.events.subscribe()
    }

    pub fn get_subagent(&self, id: &str) -> Option<Arc<dyn Subagent>> {
        self.subagents.get(id).cloned()
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
            self.logger.info(&format!("Parsed {} task(s)", tasks.len()), None);
            // 2. 計画
            let plan = self.planner.create_plan(tasks, pattern);
            self.logger
                .info(&format!("Created plan with pattern: {}", pattern_name(plan.pattern)), None);
            // 3. 実行
            let pattern = plan.pattern;
            let results = self.execute_plan(plan).await?;
            // 4. 集約
            let aggregated = self.result_aggregator.aggregate(results, pattern)?;
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
    async fn execute_task(&self, task: Task) -> Result<TaskResult> {
        self.logger.info(
            &format!("Executing task: {}", task.id),
            Some(&json!({ "action": task.action })),
        );
        let id = task.id.clone();
        self.active_tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.clone(), task.clone());
        let result = match self.select_subagent(&task) {
            Some(agent) => agent.execute(task).await,
            None => Err(Error::NoSubagent(task.action.clone())),
        };
        self.active_tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
        result
    }

    /// アクション名から Subagent を選ぶ（未知のアクションは Subagent ID とみなす）
    fn select_subagent(&self, task: &Task) -> Option<Arc<dyn Subagent>> {
        let id = match task.action.as_str() {
            "research" => "research-001",
            "codegen" => "codegen-001",
            "review" => "review-001",
            "document" => "document-001",
            other => other,
        };
        self.subagents.get(id).cloned()
    }

    /// A2A メッセージを処理。heartbeat などレスポンス不要のときは None。
    pub async fn handle_a2a_message(&self, message: &A2AMessage) -> Option<A2AMessage> {
        if let Err(e) = A2AProtocol::validate_message(message) {
            self.logger.error("Invalid A2A message", Some(&json!({ "error": e })));
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
                let str_field = |k: &str| payload.get(k).and_then(Value::as_str).unwrap_or("").to_string();
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
                        correlation_id: message.metadata.as_ref().and_then(|m| m.correlation_id.clone()),
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
        assert!(evaluate_condition(&json!("previous.results.length > 1"), &ok));
        assert!(!evaluate_condition(&json!("previous.results.length >= 3"), &ok));
        assert!(evaluate_condition(&json!("previous.results.length === 2"), &ok));
        assert!(evaluate_condition(&json!("previous.results.length =< 0"), &ok));
        assert!(evaluate_condition(&json!("anything else"), &bad));
        assert!(evaluate_condition(&json!(true), &bad));
    }
}
