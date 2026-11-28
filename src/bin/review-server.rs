//! Review Agent MCP Server
//!
//! 環境変数（`AGENT_ID` / `MAX_CONCURRENT` / `SUBAGENT_MCP_MODE` / `SUBAGENT_DEADLINE_SECS`、
//! LLM の backend と設定）は [`mcp::run_stdio`] が反映する。typed tool の `lint: true` で
//! 実行するコマンドは `SUBAGENT_REVIEW_LINT_COMMAND`。

use subagent::llm::AgentKind;
use subagent::{ReviewAgent, mcp};

#[tokio::main]
async fn main() {
    mcp::run_stdio(
        ReviewAgent::default_config(),
        AgentKind::Review,
        |c, llm, settings| {
            ReviewAgent::with_config(c)
                .with_llm(llm, settings)
                .with_lint_command(std::env::var(ReviewAgent::LINT_COMMAND_ENV).ok())
        },
        "Review",
    )
    .await;
}
