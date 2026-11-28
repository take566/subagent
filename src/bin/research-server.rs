//! Research Agent MCP Server
//!
//! 環境変数（`AGENT_ID` / `MAX_CONCURRENT` / `SUBAGENT_MCP_MODE` / `SUBAGENT_DEADLINE_SECS`、
//! LLM の backend と設定）は [`mcp::run_stdio`] が反映する。

use subagent::llm::AgentKind;
use subagent::{ResearchAgent, mcp};

#[tokio::main]
async fn main() {
    mcp::run_stdio(
        ResearchAgent::default_config(),
        AgentKind::Research,
        |c, llm, settings| ResearchAgent::with_config(c).with_llm(llm, settings),
        "Research",
    )
    .await;
}
