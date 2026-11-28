//! CodeGen Agent MCP Server
//!
//! 環境変数（`AGENT_ID` / `MAX_CONCURRENT` / `SUBAGENT_MCP_MODE` / `SUBAGENT_DEADLINE_SECS`、
//! LLM の backend と設定）は [`mcp::run_stdio`] が反映する。

use subagent::llm::AgentKind;
use subagent::{CodeGenAgent, mcp};

#[tokio::main]
async fn main() {
    mcp::run_stdio(
        CodeGenAgent::default_config(),
        AgentKind::CodeGen,
        |c, llm, settings| CodeGenAgent::with_options(c, None).with_llm(llm, settings),
        "CodeGen",
    )
    .await;
}
