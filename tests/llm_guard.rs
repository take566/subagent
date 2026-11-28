//! guard: API 鍵が env にあっても、`new()` で作ったエージェントと既定の LLM client は通信しない。
//!
//! HTTP(S)_PROXY を 127.0.0.1 の wiremock に向け、外へ出る通信を全部そこで捕まえる。
//! 捕まえられることは陽性対照（明示的に作った AnthropicClient）で確かめる。
//! env を書き換えるので、この test binary にはこの test だけを置く。

use std::sync::Arc;

use serde_json::json;
use subagent::llm::{AnthropicClient, LlmClient, LlmConfig, LlmError, Message, MessageRequest};
use subagent::{CodeGenAgent, DocumentAgent, Orchestrator, ResearchAgent, ReviewAgent, Task, llm};
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::any};

#[test]
fn keys_in_env_do_not_make_default_constructed_code_call_the_api() {
    // 先に port だけ取り、env を書き換えてから runtime と mock server を起こす
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let trap_uri = format!("http://{}", listener.local_addr().unwrap());

    // SAFETY: この binary の test はこれ 1 本で、ここではまだ tokio runtime も mock server も
    // 起こしていない（他に thread を作るものが無い）。harness の main thread は test の終わりを
    // 待つだけで env を読まない。
    unsafe {
        std::env::set_var("SUBAGENT_ANTHROPIC_API_KEY", "sk-guard-sub");
        std::env::set_var("ANTHROPIC_API_KEY", "sk-guard-generic");
        for k in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ] {
            std::env::set_var(k, &trap_uri);
        }
        std::env::remove_var("NO_PROXY");
        std::env::remove_var("no_proxy");
    }

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let trap = MockServer::builder().listener(listener).start().await;
        assert_eq!(trap.uri(), trap_uri);
        Mock::given(any())
            .respond_with(ResponseTemplate::new(599))
            .mount(&trap)
            .await;

        // 既定の client は Unconfigured
        let default = llm::default_client();
        assert!(!default.is_configured());
        let req = MessageRequest::new(16, vec![Message::user_text("x")]);
        assert!(matches!(
            default.create_message(&req).await,
            Err(LlmError::NotConfigured)
        ));

        // new() のエージェントを全部動かす（書き出し先だけ temp に向ける）
        let dir = tempfile::tempdir().unwrap();
        let mut o = Orchestrator::new();
        o.register_subagent(Arc::new(ResearchAgent::new()));
        o.register_subagent(Arc::new(CodeGenAgent::with_options(
            CodeGenAgent::default_config(),
            Some(dir.path().to_path_buf()),
        )));
        o.register_subagent(Arc::new(ReviewAgent::new()));
        o.register_subagent(Arc::new(DocumentAgent::with_options(
            DocumentAgent::default_config(),
            Some(dir.path().to_path_buf()),
        )));
        let tasks = vec![
            Task::new(
                "r",
                "research",
                json!({"query": "rust", "depth": "detailed"}),
            ),
            Task::new("c", "codegen", json!({"specification": "add two numbers"})),
            Task::new("v", "review", json!({"source_code": "fn main() {}"})),
            Task::new(
                "d",
                "document",
                json!({"content": "# T", "format": "markdown"}),
            ),
        ];
        let r = o.execute(tasks, None).await.unwrap();
        assert!(r.success, "{:?}", r.results);
        assert_eq!(
            trap.received_requests().await.unwrap().len(),
            0,
            "default-constructed code reached the network"
        );

        // 陽性対照: 明示的に作った client の通信は trap に届く
        let mut cfg = LlmConfig::new("sk-guard-sub");
        cfg.base_url = "http://api.guard.invalid".into();
        cfg.max_retries = 0;
        let explicit: Arc<AnthropicClient> = AnthropicClient::new(cfg).unwrap();
        let _ = explicit.create_message(&req).await;
        assert_eq!(
            trap.received_requests().await.unwrap().len(),
            1,
            "proxy trap did not capture the explicit client; the guard above proves nothing"
        );
    });
}
