//! 基本的な使用例
//!
//! `cargo run --example basic_usage`
//! （CodeGen / Document エージェントはカレントディレクトリにファイルを出力する）

use serde_json::json;
use subagent::{create_default_orchestrator, ExecutionPattern, Task, TaskContext};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let orchestrator = create_default_orchestrator();

    println!("=== Research Agent の使用例 ===");
    let research = orchestrator
        .execute(
            Task::new(
                "task-research-001",
                "research",
                json!({ "query": "最新のCI/CDトレンド", "depth": "detailed" }),
            ),
            None,
        )
        .await?;
    println!("Research Result: {}", serde_json::to_string_pretty(&research)?);

    println!("\n=== 並列実行の例 ===");
    let parallel = orchestrator
        .execute(
            vec![
                Task::new("task-1", "research", json!({ "query": "Rust best practices" })),
                Task::new(
                    "task-2",
                    "document",
                    json!({ "content": "# Rust Best Practices\n\n...", "format": "markdown" }),
                ),
            ],
            Some(ExecutionPattern::Parallel),
        )
        .await?;
    println!("Parallel Result: {}", serde_json::to_string_pretty(&parallel)?);

    println!("\n=== 逐次実行の例 ===");
    let mut codegen = Task::new(
        "task-seq-2",
        "codegen",
        json!({ "specification": "Implement API client based on research", "language": "typescript" }),
    );
    codegen.context = Some(TaskContext {
        parent_task: Some("task-seq-1".into()),
        ..Default::default()
    });
    let sequential = orchestrator
        .execute(
            vec![
                Task::new("task-seq-1", "research", json!({ "query": "API design patterns" })),
                codegen,
            ],
            Some(ExecutionPattern::Sequential),
        )
        .await?;
    println!("Sequential Result: {}", serde_json::to_string_pretty(&sequential)?);

    Ok(())
}
