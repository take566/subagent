//! Document Agent MCP Server

use std::sync::Arc;
use subagent::{DocumentAgent, mcp};

#[tokio::main]
async fn main() {
    mcp::run_stdio(Arc::new(DocumentAgent::new()), "Document").await;
}
