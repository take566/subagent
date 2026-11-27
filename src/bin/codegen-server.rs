//! CodeGen Agent MCP Server

use std::sync::Arc;
use subagent::{CodeGenAgent, mcp};

#[tokio::main]
async fn main() {
    mcp::run_stdio(Arc::new(CodeGenAgent::new()), "CodeGen").await;
}
