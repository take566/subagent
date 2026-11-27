//! Review Agent MCP Server

use std::sync::Arc;
use subagent::{mcp, ReviewAgent};

#[tokio::main]
async fn main() {
    mcp::run_stdio(Arc::new(ReviewAgent::new()), "Review").await;
}
