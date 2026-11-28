//! CodeGen Agent MCP Server
//!
//! 環境変数 `AGENT_ID` / `MAX_CONCURRENT` は [`mcp::run_stdio`] が反映する。

use subagent::{CodeGenAgent, mcp};

#[tokio::main]
async fn main() {
    mcp::run_stdio(
        CodeGenAgent::default_config(),
        |c| CodeGenAgent::with_options(c, None),
        "CodeGen",
    )
    .await;
}
