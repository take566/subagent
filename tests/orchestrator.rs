//! Orchestrator の結合テスト

use serde_json::{Value, json};
use std::sync::Arc;
use subagent::{
    AgentStatus, CodeGenAgent, DocumentAgent, ExecutionPattern, Orchestrator, ResearchAgent,
    ReviewAgent, Subagent, SubagentEvent, Task, TaskContext, TaskStatus,
};

/// 出力先を一時ディレクトリにした既定構成
fn orchestrator(dir: &std::path::Path) -> Orchestrator {
    let mut o = Orchestrator::new();
    o.register_subagent(Arc::new(ResearchAgent::new()));
    o.register_subagent(Arc::new(CodeGenAgent::with_options(
        CodeGenAgent::default_config(),
        Some(dir.to_path_buf()),
    )));
    o.register_subagent(Arc::new(ReviewAgent::new()));
    o.register_subagent(Arc::new(DocumentAgent::with_options(
        DocumentAgent::default_config(),
        Some(dir.to_path_buf()),
    )));
    o
}

fn with_parent(mut t: Task, parent: &str) -> Task {
    t.context = Some(TaskContext {
        parent_task: Some(parent.into()),
        ..Default::default()
    });
    t
}

#[tokio::test]
async fn single_research_task() {
    let dir = tempfile::tempdir().unwrap();
    let o = orchestrator(dir.path());
    let r = o
        .execute(
            Task::new(
                "r1",
                "research",
                json!({ "query": "CI/CD", "depth": "detailed" }),
            ),
            None,
        )
        .await
        .unwrap();
    assert!(r.success);
    assert_eq!(r.results[0].status, TaskStatus::Completed);
    let combined = &r.aggregated["combined"];
    assert_eq!(combined["confidence"], json!(0.5));
    assert!(
        combined["summary"]
            .as_str()
            .unwrap()
            .starts_with("「CI/CD」に関する調査結果")
    );
}

#[tokio::test]
async fn parallel_writes_into_base_dir() {
    let dir = tempfile::tempdir().unwrap();
    let o = orchestrator(dir.path());
    let r = o
        .execute(
            vec![
                Task::new("p1", "research", json!({ "query": "Rust" })),
                Task::new(
                    "p2",
                    "document",
                    json!({ "content": "# T", "format": "markdown" }),
                ),
                Task::new("p3", "codegen", json!({ "specification": "util" })),
            ],
            Some(ExecutionPattern::Parallel),
        )
        .await
        .unwrap();
    assert!(r.success);
    assert_eq!(r.metrics.success_count, 3);
    assert!(dir.path().join("generated.ts").exists());
    assert!(dir.path().join("generated.ts.test.ts").exists());
    let doc = r.results[1].result.as_ref().unwrap()["document_path"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(doc.ends_with(".md"));
    assert_eq!(std::fs::read_to_string(doc).unwrap(), "# T");
}

#[tokio::test]
async fn parallel_failure_lets_others_finish() {
    let dir = tempfile::tempdir().unwrap();
    let o = orchestrator(dir.path());
    let codegen = o.get_subagent("codegen-001").unwrap();
    let err = o
        .execute(
            vec![
                // document は content 必須 → 失敗
                Task::new("bad", "document", json!({})),
                Task::new("ok", "codegen", json!({ "specification": "x" })),
            ],
            Some(ExecutionPattern::Parallel),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("content"));
    // 失敗しても他のタスクは最後まで実行され、状態も戻っている
    assert_eq!(codegen.get_state().status, AgentStatus::Completed);
    assert!(codegen.get_state().current_task.is_none());
    assert!(dir.path().join("generated.ts").exists());
    assert!(o.active_task_ids().is_empty());
}

#[tokio::test]
async fn sequential_passes_previous_results() {
    let dir = tempfile::tempdir().unwrap();
    let o = orchestrator(dir.path());
    let r = o
        .execute(
            vec![
                with_parent(
                    Task::new("s2", "codegen", json!({ "specification": "client" })),
                    "s1",
                ),
                Task::new("s1", "research", json!({ "query": "API" })),
            ],
            None, // parent_task があるので sequential が自動選択される
        )
        .await
        .unwrap();
    let ids: Vec<_> = r.results.iter().map(|x| x.task_id.as_str()).collect();
    assert_eq!(ids, vec!["s1", "s2"]);
    let chain = r.aggregated["chain"].as_array().unwrap();
    assert_eq!(chain.len(), 2);
    assert!(r.aggregated["final_result"]["files"].is_array());
}

#[tokio::test]
async fn hierarchical_parent_first() {
    let dir = tempfile::tempdir().unwrap();
    let o = orchestrator(dir.path());
    let r = o
        .execute(
            vec![
                Task::new("root", "research", json!({ "query": "x" })),
                with_parent(
                    Task::new("c1", "codegen", json!({ "specification": "a" })),
                    "root",
                ),
                with_parent(
                    Task::new("c2", "review", json!({ "source_code": "let a = 1;" })),
                    "root",
                ),
            ],
            Some(ExecutionPattern::Hierarchical),
        )
        .await
        .unwrap();
    assert_eq!(r.results[0].task_id, "root");
    assert!(r.aggregated["parent"]["summary"].is_string());
    assert_eq!(r.aggregated["children"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn conditional_skips_false_conditions() {
    let dir = tempfile::tempdir().unwrap();
    let o = orchestrator(dir.path());
    let r = o
        .execute(
            vec![
                Task::new("c1", "research", json!({ "query": "x" })),
                Task::new(
                    "c2",
                    "review",
                    json!({ "source_code": "x", "condition": "previous.results.length > 5" }),
                ),
                Task::new(
                    "c3",
                    "review",
                    json!({ "source_code": "x", "condition": "previous.success" }),
                ),
            ],
            None, // condition があるので conditional が自動選択される
        )
        .await
        .unwrap();
    let ids: Vec<_> = r.results.iter().map(|x| x.task_id.as_str()).collect();
    assert_eq!(ids, vec!["c1", "c3"]);
    assert_eq!(r.aggregated["executed"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn unknown_action_and_empty_sequential_are_errors() {
    let dir = tempfile::tempdir().unwrap();
    let o = orchestrator(dir.path());
    let e = o
        .execute(Task::new("u", "nope", json!({})), None)
        .await
        .unwrap_err();
    assert_eq!(e.to_string(), "No suitable subagent found for action: nope");
    assert!(
        o.execute(Vec::<Task>::new(), Some(ExecutionPattern::Sequential))
            .await
            .is_err()
    );
    // 自由記述は execute アクション → 対応 Subagent なし
    assert!(o.execute("調べて", None).await.is_err());
}

#[tokio::test]
async fn progress_events_are_forwarded() {
    let dir = tempfile::tempdir().unwrap();
    let o = orchestrator(dir.path());
    let mut rx = o.subscribe();
    o.execute(Task::new("e1", "document", json!({ "content": "x" })), None)
        .await
        .unwrap();
    let mut progress = vec![];
    let mut statuses = vec![];
    while let Ok(ev) = rx.try_recv() {
        match ev {
            SubagentEvent::Progress { agent_id, report } => {
                assert_eq!(agent_id, "document-001");
                assert_eq!(report.task_id, "e1");
                progress.push(report.progress);
            }
            SubagentEvent::StatusChanged { status, .. } => statuses.push(status),
        }
    }
    assert_eq!(progress, vec![0.2, 0.5, 0.7, 0.9]);
    assert_eq!(statuses, vec![AgentStatus::Running, AgentStatus::Completed]);
}

#[tokio::test]
async fn failed_task_is_recorded_in_history() {
    let agent = DocumentAgent::new();
    assert!(
        agent
            .execute(Task::new("f", "document", json!({})))
            .await
            .is_err()
    );
    let s = agent.get_state();
    assert_eq!(s.status, AgentStatus::Failed);
    assert_eq!(s.context.history.len(), 1);
    let err = s.context.history[0].error.as_ref().unwrap();
    assert_eq!(err.code, "DOCUMENT_ERROR");
    assert_eq!(s.metrics.success_rate, 0.0);
}

#[tokio::test]
async fn review_runs_lint_command_when_given() {
    let agent = ReviewAgent::new();
    let r = agent
        .execute(Task::new(
            "l",
            "review",
            // 先頭が `/` だと（TS 版と同じく）ファイルパス扱いになるので、コメントは行末に置く
            json!({ "source_code": "x // c", "lint_command": "echo 1 error here" }),
        ))
        .await
        .unwrap();
    let res = r.result.unwrap();
    assert_eq!(res["issues"][0]["type"], "error");
    assert_eq!(res["score"], 90);
}

#[tokio::test]
async fn a2a_task_request_roundtrip() {
    use subagent::{A2AProtocol, MessageType, TaskRequestPayload};
    let dir = tempfile::tempdir().unwrap();
    let o = orchestrator(dir.path());
    let req = A2AProtocol::create_task_request(
        "client",
        "orchestrator",
        &TaskRequestPayload {
            task_id: "a1".into(),
            action: "research".into(),
            parameters: json!({ "query": "q" }).as_object().cloned().unwrap(),
            context: None,
        },
        None,
        Some("corr".into()),
    );
    let resp = o.handle_a2a_message(&req).await.unwrap();
    assert_eq!(resp.message_type, Some(MessageType::TaskResult));
    assert_eq!(resp.in_reply_to.as_deref(), Some(req.message_id.as_str()));
    assert_eq!(resp.status, Some(TaskStatus::Completed));
    assert_eq!(resp.payload.unwrap()["task_id"], Value::from("a1"));

    let hb = A2AProtocol::create_heartbeat("client", "orchestrator");
    assert!(o.handle_a2a_message(&hb).await.is_none());

    let mut bad = req.clone();
    bad.to.clear();
    let e = o.handle_a2a_message(&bad).await.unwrap();
    assert_eq!(e.message_type, Some(MessageType::Error));
}

// ---------------------------------------------------------------------------
// テスト用エージェント: 受け取った context を結果として返す / 終わらない
// ---------------------------------------------------------------------------

mod probe {
    use async_trait::async_trait;
    use serde_json::{Value, json};
    use subagent::utils::ErrorHandler;
    use subagent::utils::Logger;
    use subagent::{
        AgentBase, BehaviorConfig, InterfaceConfig, Result, RoleConfig, Subagent, SubagentConfig,
        Task, TaskResult,
    };

    pub struct Probe {
        base: AgentBase,
        handler: ErrorHandler,
        hang: bool,
    }

    impl Probe {
        pub fn new(id: &str, hang: bool) -> Self {
            let config = SubagentConfig {
                id: id.into(),
                name: id.into(),
                version: "0".into(),
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
            };
            Probe {
                base: AgentBase::new(config, vec![]),
                handler: ErrorHandler::new(Default::default(), Logger::new("probe")),
                hang,
            }
        }
    }

    #[async_trait]
    impl Subagent for Probe {
        fn base(&self) -> &AgentBase {
            &self.base
        }

        async fn execute(&self, task: Task) -> Result<TaskResult> {
            let hang = self.hang;
            self.base
                .run_task(&task, &self.handler, "PROBE_ERROR", || async {
                    if hang {
                        std::future::pending::<()>().await;
                    }
                    let ctx = serde_json::to_value(&task.context).unwrap_or(Value::Null);
                    Ok((json!({ "seen_context": ctx, "id": task.id }), 1.0, 0))
                })
                .await
        }
    }
}

#[tokio::test]
async fn sequential_previous_results_content() {
    let mut o = Orchestrator::new();
    o.register_subagent(Arc::new(probe::Probe::new("echo", false)));
    let r = o
        .execute(
            vec![
                Task::new("a", "echo", json!({})),
                with_parent(Task::new("b", "echo", json!({})), "a"),
            ],
            Some(ExecutionPattern::Sequential),
        )
        .await
        .unwrap();
    let seen = &r.results[1].result.as_ref().unwrap()["seen_context"];
    assert_eq!(seen["parent_task"], "a");
    let prev = seen["previous_results"].as_array().unwrap();
    assert_eq!(prev.len(), 1);
    assert_eq!(prev[0]["taskId"], "a");
    assert_eq!(prev[0]["result"]["id"], "a");
    // 1件目には依存がないので previous_results は無い
    assert!(r.results[0].result.as_ref().unwrap()["seen_context"].is_null());
}

#[tokio::test]
async fn hierarchical_children_see_parent_result() {
    let mut o = Orchestrator::new();
    o.register_subagent(Arc::new(probe::Probe::new("echo", false)));
    let r = o
        .execute(
            vec![
                Task::new("root", "echo", json!({})),
                with_parent(Task::new("kid", "echo", json!({})), "root"),
            ],
            Some(ExecutionPattern::Hierarchical),
        )
        .await
        .unwrap();
    let seen = &r.results[1].result.as_ref().unwrap()["seen_context"];
    assert_eq!(seen["parent_result"]["id"], "root");
}

#[tokio::test]
async fn cancelled_task_resets_agent_state() {
    let agent = Arc::new(probe::Probe::new("hang", true));
    let t = tokio::time::timeout(
        std::time::Duration::from_millis(50),
        agent.execute(Task::new("h", "hang", json!({}))),
    )
    .await;
    assert!(t.is_err(), "should time out");
    let s = agent.get_state();
    assert_eq!(s.status, AgentStatus::Idle);
    assert!(s.current_task.is_none());
}

#[tokio::test]
async fn review_requires_string_source_code() {
    let agent = ReviewAgent::new();
    for params in [
        json!({}),
        json!({ "source_code": 42 }),
        json!({ "source_code": null }),
    ] {
        let e = agent
            .execute(Task::new("r", "review", params))
            .await
            .unwrap_err();
        assert!(e.to_string().contains("source_code"), "{e}");
    }
    assert_eq!(agent.get_state().metrics.success_rate, 0.0);
}

#[tokio::test]
async fn codegen_and_document_param_types_like_ts() {
    let dir = tempfile::tempdir().unwrap();
    let cg = CodeGenAgent::with_options(CodeGenAgent::default_config(), Some(dir.path().into()));
    // language が null なら TS の既定値は効かず .js になる
    cg.execute(Task::new(
        "c",
        "codegen",
        json!({ "specification": "s", "language": null }),
    ))
    .await
    .unwrap();
    assert!(dir.path().join("generated.js").exists());

    let doc = DocumentAgent::with_options(DocumentAgent::default_config(), Some(dir.path().into()));
    assert!(
        doc.execute(Task::new(
            "d",
            "document",
            json!({ "content": "x", "format": null })
        ))
        .await
        .is_err()
    );
}
