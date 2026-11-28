//! エージェントごとの LLM 設定（[`AgentLlmSettings`]）と、bin が使う client の選択（[`client_from_env`]）
//!
//! env（どれも任意。`<AGENT>` は `RESEARCH` / `CODEGEN` / `REVIEW` / `DOCUMENT`）:
//! - `SUBAGENT_[<AGENT>_]MODEL`: model（既定 `claude-opus-5-5`）
//! - `SUBAGENT_[<AGENT>_]EFFORT`: `low` / `medium` / `high` / `xhigh` / `max`（既定は送らない）
//! - `SUBAGENT_[<AGENT>_]TOKEN_BUDGET`: 1 タスクの token 予算（正の整数）
//!
//! エージェント名付きの値が全体の値より優先する。不正な値はエラーにする（bin は起動しない）。
//!
//! backend:
//! - `SUBAGENT_LLM_BACKEND`: 未設定・空・`anthropic` なら [`super::shared_from_env`]（鍵が無ければ
//!   [`super::UnconfiguredLlm`]）。`replay` は feature `replay` 付きの build だけで、
//!   `SUBAGENT_LLM_REPLAY_FILE` の記録を返す。それ以外の値はエラー（fail closed）

use std::sync::Arc;

use tokio::time::Instant;

use super::client::{LlmClient, LlmError};
use super::config::TokenBudget;
use super::runner::RunnerLimits;
use super::types::{
    CacheControl, DEFAULT_MODEL, Effort, MessageRequest, OutputConfig, ThinkingConfig, ToolDef,
};

/// backend を選ぶ env
pub const LLM_BACKEND_ENV: &str = "SUBAGENT_LLM_BACKEND";
/// replay backend の記録ファイルの env
pub const LLM_REPLAY_FILE_ENV: &str = "SUBAGENT_LLM_REPLAY_FILE";

/// どのエージェントか（env の名前と既定値を決める）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentKind {
    Research,
    CodeGen,
    Review,
    Document,
}

impl AgentKind {
    /// env の `<AGENT>` 部分
    pub fn env_name(self) -> &'static str {
        match self {
            AgentKind::Research => "RESEARCH",
            AgentKind::CodeGen => "CODEGEN",
            AgentKind::Review => "REVIEW",
            AgentKind::Document => "DOCUMENT",
        }
    }
}

/// エージェント 1 つ分の LLM 設定
#[derive(Debug, Clone, PartialEq)]
pub struct AgentLlmSettings {
    pub model: String,
    /// 1 応答の max_tokens（runner は max_tokens で tool_use が切れた時だけ 1 回増やす）
    pub max_tokens: u32,
    /// `output_config.effort`。None は送らない（API の既定）
    pub effort: Option<Effort>,
    /// 1 タスクの token 予算
    pub token_budget: u64,
    /// 1 タスクで API を呼ぶ回数の上限
    pub max_iterations: u32,
    /// top-level の `cache_control` を付けるか（1 往復で終わる review / document は付けない）
    pub cache: bool,
}

impl AgentLlmSettings {
    /// エージェントごとの既定値
    pub fn defaults(kind: AgentKind) -> Self {
        let (token_budget, max_iterations, cache) = match kind {
            AgentKind::Research => (TokenBudget::research().limit(), 10, true),
            AgentKind::CodeGen => (TokenBudget::codegen().limit(), 25, true),
            AgentKind::Review => (TokenBudget::review().limit(), 3, false),
            AgentKind::Document => (TokenBudget::document().limit(), 3, false),
        };
        // review は 1 回の応答に指摘を全部入れるので広めに取る（runner の上限 32000 と同じ）
        let max_tokens = match kind {
            AgentKind::Review => 32_000,
            _ => 16_000,
        };
        Self {
            model: DEFAULT_MODEL.to_string(),
            max_tokens,
            effort: None,
            token_budget,
            max_iterations,
            cache,
        }
    }

    /// process の env から読む
    pub fn from_env(kind: AgentKind) -> Result<Self, LlmError> {
        Self::from_lookup(kind, |k| std::env::var(k).ok())
    }

    /// 任意の lookup から読む（テスト用）
    pub fn from_lookup(
        kind: AgentKind,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, LlmError> {
        let mut s = Self::defaults(kind);
        // `SUBAGENT_<AGENT>_X` を `SUBAGENT_X` より優先する。空は未設定と同じ
        let get = |suffix: &str| -> Option<(String, String)> {
            [
                format!("SUBAGENT_{}_{suffix}", kind.env_name()),
                format!("SUBAGENT_{suffix}"),
            ]
            .into_iter()
            .find_map(|name| {
                lookup(&name)
                    .map(|v| v.trim().to_string())
                    .filter(|v| !v.is_empty())
                    .map(|v| (name, v))
            })
        };
        if let Some((_, model)) = get("MODEL") {
            s.model = model;
        }
        if let Some((name, raw)) = get("EFFORT") {
            s.effort = Some(parse_effort(&raw).ok_or_else(|| {
                LlmError::InvalidConfig(format!(
                    "{name} must be one of low, medium, high, xhigh, max"
                ))
            })?);
        }
        if let Some((name, raw)) = get("TOKEN_BUDGET") {
            s.token_budget = raw.parse().ok().filter(|n| *n > 0).ok_or_else(|| {
                LlmError::InvalidConfig(format!("{name} must be a positive integer"))
            })?;
        }
        Ok(s)
    }

    /// model / max_tokens / thinking（adaptive）/ effort / cache を入れた request の雛形。
    /// sampling の値（temperature / top_p / top_k）と tool_choice は送らない
    pub fn request_template(&self, system: Option<String>, tools: Vec<ToolDef>) -> MessageRequest {
        let mut req = MessageRequest::new(self.max_tokens, Vec::new());
        req.model = self.model.clone();
        req.system = system;
        req.tools = tools;
        req.thinking = Some(ThinkingConfig::default());
        req.output_config = self.effort.map(|effort| OutputConfig {
            effort: Some(effort),
            format: None,
        });
        req.cache_control = self.cache.then(CacheControl::ephemeral);
        req
    }

    /// 1 タスク分の上限（新しい token 予算と deadline）。送る前に予算を確かめる
    /// （[`RunnerLimits::check_budget_before_send`]）
    pub fn runner_limits(&self, deadline: Instant) -> RunnerLimits {
        RunnerLimits {
            max_iterations: self.max_iterations,
            token_budget: Some(Arc::new(TokenBudget::new(self.token_budget))),
            deadline: Some(deadline),
            check_budget_before_send: true,
            ..RunnerLimits::default()
        }
    }
}

fn parse_effort(s: &str) -> Option<Effort> {
    Some(match s.to_ascii_lowercase().as_str() {
        "low" => Effort::Low,
        "medium" => Effort::Medium,
        "high" => Effort::High,
        "xhigh" => Effort::Xhigh,
        "max" => Effort::Max,
        _ => return None,
    })
}

/// エージェントが持つ LLM（client と設定）
#[derive(Clone)]
pub struct AgentLlm {
    pub client: Arc<dyn LlmClient>,
    pub settings: AgentLlmSettings,
}

impl std::fmt::Debug for AgentLlm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentLlm")
            .field("configured", &self.client.is_configured())
            .field("settings", &self.settings)
            .finish()
    }
}

impl AgentLlm {
    pub fn new(client: Arc<dyn LlmClient>, settings: AgentLlmSettings) -> Self {
        Self { client, settings }
    }

    /// 通信しない既定（[`super::UnconfiguredLlm`] と既定の設定）
    pub fn unconfigured(kind: AgentKind) -> Self {
        Self::new(super::default_client(), AgentLlmSettings::defaults(kind))
    }

    /// 呼べる client か。未設定なら [`LlmError::NotConfigured`]（typed tool は mock の出力を返さない）
    pub fn ensure_configured(&self) -> Result<(), LlmError> {
        if self.client.is_configured() {
            Ok(())
        } else {
            Err(LlmError::NotConfigured)
        }
    }
}

/// どの backend を使うか
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmBackend {
    /// Claude API（鍵が無ければ Unconfigured）
    Anthropic,
    /// 記録した応答を返す（feature `replay` 付きの build だけ）
    Replay { file: String },
}

/// `SUBAGENT_LLM_BACKEND` / `SUBAGENT_LLM_REPLAY_FILE` を読む。不明な値と、`replay` feature 無しの
/// build での `replay` はエラー
pub fn select_backend(lookup: impl Fn(&str) -> Option<String>) -> Result<LlmBackend, LlmError> {
    let raw = lookup(LLM_BACKEND_ENV).unwrap_or_default();
    match raw.trim() {
        "" | "anthropic" => Ok(LlmBackend::Anthropic),
        "replay" if cfg!(feature = "replay") => {
            let file = lookup(LLM_REPLAY_FILE_ENV)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .ok_or_else(|| {
                    LlmError::InvalidConfig(format!(
                        "{LLM_BACKEND_ENV}=replay needs {LLM_REPLAY_FILE_ENV}"
                    ))
                })?;
            Ok(LlmBackend::Replay { file })
        }
        "replay" => Err(LlmError::InvalidConfig(format!(
            "{LLM_BACKEND_ENV}=replay is only available in a build with the `replay` feature"
        ))),
        _ => Err(LlmError::InvalidConfig(format!(
            "{LLM_BACKEND_ENV} must be empty, `anthropic` or `replay`"
        ))),
    }
}

/// bin が使う process 共有の client（[`select_backend`] で選ぶ）
pub fn client_from_env() -> Result<Arc<dyn LlmClient>, LlmError> {
    match select_backend(|k| std::env::var(k).ok())? {
        LlmBackend::Anthropic => super::shared_from_env(),
        #[cfg(feature = "replay")]
        LlmBackend::Replay { file } => Ok(Arc::new(super::ReplayLlm::from_file(file)?)),
        #[cfg(not(feature = "replay"))]
        LlmBackend::Replay { .. } => {
            unreachable!("select_backend rejects replay without the feature")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn lookup(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| m.get(k).cloned()
    }

    #[test]
    fn per_agent_defaults() {
        let r = AgentLlmSettings::defaults(AgentKind::Research);
        assert_eq!(r.model, "claude-opus-5-5");
        assert_eq!(r.token_budget, 300_000);
        assert!(r.cache);
        assert_eq!(
            AgentLlmSettings::defaults(AgentKind::CodeGen).token_budget,
            1_000_000
        );
        assert_eq!(
            AgentLlmSettings::defaults(AgentKind::CodeGen).max_iterations,
            25
        );
        // 1 往復の review / document は cache を付けない
        assert!(!AgentLlmSettings::defaults(AgentKind::Review).cache);
        assert!(!AgentLlmSettings::defaults(AgentKind::Document).cache);
        assert_eq!(
            AgentLlmSettings::from_lookup(AgentKind::Review, lookup(&[])).unwrap(),
            AgentLlmSettings::defaults(AgentKind::Review)
        );
    }

    #[test]
    fn agent_specific_env_overrides_global_env() {
        let s = AgentLlmSettings::from_lookup(
            AgentKind::Research,
            lookup(&[
                ("SUBAGENT_MODEL", "global-model"),
                ("SUBAGENT_RESEARCH_MODEL", " research-model "),
                ("SUBAGENT_EFFORT", "LOW"),
                ("SUBAGENT_CODEGEN_EFFORT", "max"),
                ("SUBAGENT_TOKEN_BUDGET", "5000"),
                ("SUBAGENT_RESEARCH_TOKEN_BUDGET", ""),
            ]),
        )
        .unwrap();
        assert_eq!(s.model, "research-model");
        assert_eq!(s.effort, Some(Effort::Low));
        assert_eq!(s.token_budget, 5000);
    }

    #[test]
    fn invalid_values_name_the_env() {
        for (k, v) in [
            ("SUBAGENT_EFFORT", "extreme"),
            ("SUBAGENT_DOCUMENT_EFFORT", "1"),
            ("SUBAGENT_TOKEN_BUDGET", "0"),
            ("SUBAGENT_DOCUMENT_TOKEN_BUDGET", "-5"),
        ] {
            let e =
                AgentLlmSettings::from_lookup(AgentKind::Document, lookup(&[(k, v)])).unwrap_err();
            assert!(e.to_string().contains(k), "{k}: {e}");
        }
    }

    #[test]
    fn request_template_has_no_sampling_params() {
        let mut s = AgentLlmSettings::defaults(AgentKind::Review);
        s.effort = Some(Effort::High);
        let req = serde_json::to_value(s.request_template(Some("sys".into()), vec![])).unwrap();
        assert_eq!(req["model"], "claude-opus-5-5");
        assert_eq!(req["thinking"], serde_json::json!({"type": "adaptive"}));
        assert_eq!(req["output_config"]["effort"], "high");
        assert_eq!(req["system"], "sys");
        for k in [
            "temperature",
            "top_p",
            "top_k",
            "tool_choice",
            "cache_control",
            "tools",
        ] {
            assert!(req.get(k).is_none(), "{k}: {req}");
        }
        let req = serde_json::to_value(
            AgentLlmSettings::defaults(AgentKind::CodeGen).request_template(None, vec![]),
        )
        .unwrap();
        assert_eq!(req["cache_control"]["type"], "ephemeral");
        assert!(req.get("output_config").is_none());
    }

    #[test]
    fn backend_selection_fails_closed() {
        assert_eq!(select_backend(lookup(&[])).unwrap(), LlmBackend::Anthropic);
        assert_eq!(
            select_backend(lookup(&[(LLM_BACKEND_ENV, " anthropic ")])).unwrap(),
            LlmBackend::Anthropic
        );
        for bad in ["openai", "Replay ", "ANTHROPIC", "mock"] {
            assert!(
                select_backend(lookup(&[(LLM_BACKEND_ENV, bad)])).is_err(),
                "{bad}"
            );
        }
        let replay = select_backend(lookup(&[(LLM_BACKEND_ENV, "replay")]));
        assert!(replay.is_err());
        let with_file = select_backend(lookup(&[
            (LLM_BACKEND_ENV, "replay"),
            (LLM_REPLAY_FILE_ENV, "f.json"),
        ]));
        if cfg!(feature = "replay") {
            assert_eq!(
                with_file.unwrap(),
                LlmBackend::Replay {
                    file: "f.json".into()
                }
            );
        } else {
            assert!(with_file.unwrap_err().to_string().contains("feature"));
        }
    }

    #[test]
    fn unconfigured_agent_llm_refuses() {
        let llm = AgentLlm::unconfigured(AgentKind::Research);
        assert!(matches!(
            llm.ensure_configured(),
            Err(LlmError::NotConfigured)
        ));
    }
}
