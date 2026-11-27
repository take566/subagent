//! Subagent 基本トレイトと共通状態
//!
//! TS 版の抽象クラス `Subagent extends EventEmitter` を、
//! 共通の状態・ツール・イベント配信を持つ [`AgentBase`] と、
//! 各エージェントが実装する [`Subagent`] トレイトに分けている。

use crate::core::tool::{Tool, ToolRegistry};
use crate::error::{Error, Result};
use crate::types::{
    AgentStatus, JsonMap, ProgressReport, SubagentConfig, SubagentContextState, SubagentMetrics,
    SubagentState, Task, TaskMetrics, TaskResult, TaskStatus, now_iso,
};
use crate::utils::error_handler::ErrorHandler;
use crate::utils::logger::Logger;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;
use tokio::sync::broadcast;

/// Subagent が発行するイベント（TS の `progress` / `status_changed`）
#[derive(Debug, Clone, PartialEq)]
pub enum SubagentEvent {
    Progress {
        agent_id: String,
        report: ProgressReport,
    },
    StatusChanged {
        agent_id: String,
        status: AgentStatus,
    },
}

/// エージェント1つ分の共通部分
pub struct AgentBase {
    config: SubagentConfig,
    tools: ToolRegistry,
    state: Mutex<SubagentState>,
    events: broadcast::Sender<SubagentEvent>,
    listeners: Mutex<Vec<broadcast::Sender<SubagentEvent>>>,
    pub logger: Logger,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // パニックで poison されても状態は読み書きできるようにする
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl AgentBase {
    pub fn new(config: SubagentConfig, tools: Vec<Arc<dyn Tool>>) -> Self {
        let mut registry = ToolRegistry::new();
        for t in tools {
            registry.register(t);
        }
        let state = SubagentState {
            id: config.id.clone(),
            status: AgentStatus::Idle,
            current_task: None,
            context: SubagentContextState {
                memory: Default::default(),
                history: vec![],
                shared_context: None,
            },
            metrics: SubagentMetrics {
                tasks_completed: 0,
                avg_duration: 0.0,
                success_rate: 1.0,
            },
        };
        let (events, _) = broadcast::channel(256);
        AgentBase {
            logger: Logger::new(config.id.clone()),
            config,
            tools: registry,
            state: Mutex::new(state),
            events,
            listeners: Mutex::new(vec![]),
        }
    }

    pub fn id(&self) -> &str {
        &self.config.id
    }

    pub fn name(&self) -> &str {
        &self.config.name
    }

    /// イベントを購読
    pub fn subscribe(&self) -> broadcast::Receiver<SubagentEvent> {
        self.events.subscribe()
    }

    /// イベントの転送先を追加（Orchestrator が使う）
    pub fn add_listener(&self, sender: broadcast::Sender<SubagentEvent>) {
        lock(&self.listeners).push(sender);
    }

    fn emit(&self, event: SubagentEvent) {
        for l in lock(&self.listeners).iter() {
            let _ = l.send(event.clone());
        }
        let _ = self.events.send(event);
    }

    /// ツールを呼び出し
    pub async fn call_tool(&self, name: &str, params: JsonMap) -> Result<Value> {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| Error::ToolNotAvailable {
                tool: name.into(),
                agent: self.config.name.clone(),
            })?;
        if !self.config.tools.iter().any(|t| t == name) {
            return Err(Error::ToolNotAllowed {
                tool: name.into(),
                agent: self.config.name.clone(),
            });
        }
        self.logger.debug(
            &format!("Calling tool: {name}"),
            Some(&json!({ "params": params })),
        );
        match tool.invoke(params).await {
            Ok(result) => {
                self.logger.debug(
                    &format!("Tool {name} completed"),
                    Some(&json!({ "result": result })),
                );
                Ok(result)
            }
            Err(e) => {
                self.logger.error(
                    &format!("Tool {name} failed"),
                    Some(&json!({ "error": e.to_string() })),
                );
                Err(e)
            }
        }
    }

    /// 進捗を報告。並列実行でも正しいタスクに紐づくよう task_id を明示的に受け取る。
    pub fn report_progress(&self, task_id: &str, progress: f64, message: &str) {
        let report = ProgressReport {
            task_id: task_id.into(),
            progress: progress.clamp(0.0, 1.0),
            message: message.into(),
            timestamp: now_iso(),
        };
        self.logger.info(
            &format!("Progress: {}%", (progress * 100.0).round() as i64),
            Some(&json!({ "message": message })),
        );
        self.emit(SubagentEvent::Progress {
            agent_id: self.config.id.clone(),
            report,
        });
    }

    /// 状態を更新
    pub fn update_status(&self, status: AgentStatus) {
        lock(&self.state).status = status;
        self.emit(SubagentEvent::StatusChanged {
            agent_id: self.config.id.clone(),
            status,
        });
    }

    fn set_current_task(&self, task: Option<Task>) {
        lock(&self.state).current_task = task;
    }

    pub fn set_memory(&self, key: impl Into<String>, value: Value) {
        lock(&self.state).context.memory.insert(key.into(), value);
    }

    pub fn get_memory(&self, key: &str) -> Option<Value> {
        lock(&self.state).context.memory.get(key).cloned()
    }

    /// 履歴に追加してメトリクスを更新
    pub fn add_to_history(&self, result: TaskResult) {
        let mut state = lock(&self.state);
        let duration = result.metrics.as_ref().map(|m| m.duration_ms).unwrap_or(0);
        state.context.history.push(result);

        let m = &mut state.metrics;
        m.tasks_completed += 1;
        // TS: `if (result.metrics?.duration_ms)` — 0ms は平均に含めない
        if duration > 0 {
            let total = m.avg_duration * (m.tasks_completed - 1) as f64 + duration as f64;
            m.avg_duration = total / m.tasks_completed as f64;
        }
        let success = state
            .context
            .history
            .iter()
            .filter(|r| r.status == TaskStatus::Completed)
            .count();
        state.metrics.success_rate = success as f64 / state.metrics.tasks_completed as f64;
    }

    /// 現在の状態のスナップショット
    pub fn get_state(&self) -> SubagentState {
        lock(&self.state).clone()
    }

    pub fn get_config(&self) -> &SubagentConfig {
        &self.config
    }

    pub fn get_available_tools(&self) -> Vec<String> {
        self.tools.get_names()
    }

    pub fn get_tool(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name)
    }

    /// ツールを呼び出し（MCP 統合用）
    pub async fn invoke_tool(&self, name: &str, params: JsonMap) -> Result<Value> {
        self.call_tool(name, params).await
    }

    /// 全エージェント共通のタスク実行ライフサイクル。
    ///
    /// 状態を running にし、`body` を実行して成功なら completed の結果を、
    /// 失敗なら failed の結果を履歴に積んでからエラーを返す（TS 版と同じ挙動）。
    /// `body` は `(result, confidence, tools_called)` を返す。
    pub async fn run_task<F, Fut>(
        &self,
        task: &Task,
        error_handler: &ErrorHandler,
        error_code: &str,
        body: F,
    ) -> Result<TaskResult>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<(Value, f64, u64)>>,
    {
        let start = Instant::now();
        self.update_status(AgentStatus::Running);
        self.set_current_task(Some(task.clone()));

        // future が途中で drop された（キャンセルされた）場合に状態を戻す
        let mut guard = CancelGuard {
            base: self,
            task_id: &task.id,
            armed: true,
        };
        let outcome = body().await;
        guard.armed = false;
        let duration_ms = start.elapsed().as_millis() as u64;

        match outcome {
            Ok((value, confidence, tools_called)) => {
                let result = TaskResult {
                    task_id: task.id.clone(),
                    status: TaskStatus::Completed,
                    result: Some(value),
                    error: None,
                    confidence: Some(confidence),
                    metrics: Some(TaskMetrics {
                        duration_ms,
                        tools_called: Some(tools_called),
                        ..Default::default()
                    }),
                };
                self.add_to_history(result.clone());
                self.update_status(AgentStatus::Completed);
                self.set_current_task(None);
                Ok(result)
            }
            Err(e) => {
                let failed = TaskResult {
                    task_id: task.id.clone(),
                    status: TaskStatus::Failed,
                    result: None,
                    error: Some(error_handler.create_error_info(&e, error_code)),
                    confidence: None,
                    metrics: Some(TaskMetrics {
                        duration_ms,
                        ..Default::default()
                    }),
                };
                self.add_to_history(failed);
                self.update_status(AgentStatus::Failed);
                self.set_current_task(None);
                Err(e)
            }
        }
    }
}

struct CancelGuard<'a> {
    base: &'a AgentBase,
    task_id: &'a str,
    armed: bool,
}

impl Drop for CancelGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.base.logger.warn(
                "Task cancelled before completion",
                Some(&json!({ "task_id": self.task_id })),
            );
            self.base.set_current_task(None);
            self.base.update_status(AgentStatus::Idle);
        }
    }
}

/// Subagent（各エージェントが実装する）
#[async_trait]
pub trait Subagent: Send + Sync {
    fn base(&self) -> &AgentBase;

    /// タスクを実行
    async fn execute(&self, task: Task) -> Result<TaskResult>;

    fn id(&self) -> &str {
        self.base().id()
    }

    fn get_config(&self) -> &SubagentConfig {
        self.base().get_config()
    }

    fn get_state(&self) -> SubagentState {
        self.base().get_state()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{BehaviorConfig, InterfaceConfig, RoleConfig};

    fn base() -> AgentBase {
        AgentBase::new(
            SubagentConfig {
                id: "a".into(),
                name: "A".into(),
                version: "1".into(),
                role: RoleConfig {
                    description: String::new(),
                    capabilities: vec![],
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
        )
    }

    fn res(status: TaskStatus, ms: u64) -> TaskResult {
        TaskResult {
            task_id: "t".into(),
            status,
            result: None,
            error: None,
            confidence: None,
            metrics: Some(TaskMetrics {
                duration_ms: ms,
                ..Default::default()
            }),
        }
    }

    #[test]
    fn metrics_update() {
        let b = base();
        b.add_to_history(res(TaskStatus::Completed, 100));
        b.add_to_history(res(TaskStatus::Failed, 300));
        let s = b.get_state();
        assert_eq!(s.metrics.tasks_completed, 2);
        assert_eq!(s.metrics.avg_duration, 200.0);
        assert_eq!(s.metrics.success_rate, 0.5);
    }

    #[tokio::test]
    async fn unknown_tool_errors() {
        let b = base();
        let e = b.call_tool("nope", JsonMap::new()).await.unwrap_err();
        assert_eq!(e.to_string(), "Tool nope not available for A");
    }

    #[test]
    fn events_reach_listener() {
        let b = base();
        let (tx, mut rx) = broadcast::channel(8);
        b.add_listener(tx);
        b.update_status(AgentStatus::Running);
        b.report_progress("t1", 1.5, "done");
        assert_eq!(
            rx.try_recv().unwrap(),
            SubagentEvent::StatusChanged {
                agent_id: "a".into(),
                status: AgentStatus::Running
            }
        );
        match rx.try_recv().unwrap() {
            SubagentEvent::Progress { report, .. } => {
                assert_eq!(report.task_id, "t1");
                assert_eq!(report.progress, 1.0);
            }
            e => panic!("unexpected {e:?}"),
        }
    }
}
