//! Document Agent MCP Server
//!
//! 環境変数（`AGENT_ID` / `MAX_CONCURRENT` / `SUBAGENT_MCP_MODE` / `SUBAGENT_DEADLINE_SECS`、
//! LLM の backend と設定）は [`mcp::run_stdio`] が反映する。

use subagent::llm::AgentKind;
use subagent::{DocumentAgent, mcp};

#[tokio::main]
async fn main() {
    mcp::run_stdio(
        DocumentAgent::default_config(),
        AgentKind::Document,
        |c, llm, settings| DocumentAgent::with_options(c, None).with_llm(llm, settings),
        "Document",
    )
    .await;
}
