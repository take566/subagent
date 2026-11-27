//! Research Agent MCP Server

use std::sync::Arc;
use subagent::{ResearchAgent, mcp};

#[tokio::main]
async fn main() {
    mcp::run_stdio(Arc::new(ResearchAgent::new()), "Research").await;
}
