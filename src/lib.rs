//! Subagent システム
//!
//! メインエージェント（[`Orchestrator`]）から委譲されたタスクを、
//! 専門の Subagent（Research / CodeGen / Review / Document）が実行する。

pub mod agents;
pub mod core;
pub mod error;
pub mod mcp;
pub mod orchestrator;
pub mod protocol;
pub mod sandbox;
pub mod tools;
pub mod types;
pub mod utils;

pub use crate::agents::{CodeGenAgent, DocumentAgent, ResearchAgent, ReviewAgent};
pub use crate::core::{AgentBase, Subagent, SubagentEvent, Tool, ToolRegistry};
pub use crate::error::{Error, Result};
pub use crate::orchestrator::{AggregatedResult, Orchestrator, TaskInput};
pub use crate::protocol::A2AProtocol;
pub use crate::types::*;

use std::sync::Arc;

/// 4種類の既定 Subagent を登録した Orchestrator を作成
pub fn create_default_orchestrator() -> Orchestrator {
    let mut o = Orchestrator::new();
    o.register_subagent(Arc::new(ResearchAgent::new()));
    o.register_subagent(Arc::new(CodeGenAgent::new()));
    o.register_subagent(Arc::new(ReviewAgent::new()));
    o.register_subagent(Arc::new(DocumentAgent::new()));
    o
}
