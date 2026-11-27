//! Document Agent MCP Server
//!
//! 環境変数 `AGENT_ID` / `MAX_CONCURRENT` は [`mcp::run_stdio`] が反映する。

use subagent::{DocumentAgent, mcp};

#[tokio::main]
async fn main() {
    mcp::run_stdio(
        DocumentAgent::default_config(),
        |c| DocumentAgent::with_options(c, None),
        "Document",
    )
    .await;
}
