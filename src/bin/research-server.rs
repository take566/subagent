//! Research Agent MCP Server

use std::sync::Arc;
use subagent::{mcp, ResearchAgent};

#[tokio::main]
async fn main() {
    mcp::run_stdio(Arc::new(ResearchAgent::new()), "Research").await;
}
