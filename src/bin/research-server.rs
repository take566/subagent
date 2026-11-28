//! Research Agent MCP Server
//!
//! 環境変数 `AGENT_ID` / `MAX_CONCURRENT` は [`mcp::run_stdio`] が反映する。

use subagent::{ResearchAgent, mcp};

#[tokio::main]
async fn main() {
    mcp::run_stdio(
        ResearchAgent::default_config(),
        ResearchAgent::with_config,
        "Research",
    )
    .await;
}
