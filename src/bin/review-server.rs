//! Review Agent MCP Server
//!
//! 環境変数 `AGENT_ID` / `MAX_CONCURRENT` は [`mcp::run_stdio`] が反映する。

use subagent::{ReviewAgent, mcp};

#[tokio::main]
async fn main() {
    mcp::run_stdio(
        ReviewAgent::default_config(),
        ReviewAgent::with_config,
        "Review",
    )
    .await;
}
