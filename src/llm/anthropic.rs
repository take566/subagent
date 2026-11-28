//! Claude Messages API を raw HTTP（reqwest）で呼ぶ client
//!
//! - header: `x-api-key` / `anthropic-version: 2026-10-21` / `content-type: application/json`
//! - 1 process で 1 client を [`Arc`] で共有し、[`Semaphore`] で同時送信数を絞る
//! - retry は client の中だけ: 429（spend cap 以外、`retry-after` を待つ）/ 500 / 529 / その他 5xx /
//!   接続失敗。400 / 401 / 402 / 403 / 404 / 413 と spend cap の 429 は retry しない。
//!   送信後の timeout や通信断は二重課金を避けるため retry しない。
//! - process 単位の 1 日の token 上限を超えたら送信しない。

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{NaiveDate, Utc};
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue, RETRY_AFTER};
use serde_json::Value;
use tokio::sync::Semaphore;

use super::client::{BudgetScope, LlmClient, LlmError, UnconfiguredLlm};
use super::config::{ANTHROPIC_VERSION, LlmConfig};
use super::types::{MessageRequest, MessageResponse};

/// spend cap の 429 で `error.details.error_code` に入る値
const SPEND_CAP_ERROR_CODE: &str = "enforced_spend_limit_reached";
/// 非 JSON の error body を message に入れる時の上限
const MAX_ERROR_BODY_CHARS: usize = 500;

/// process 単位の 1 日の token 使用量（UTC の日付で区切る）
#[derive(Debug)]
struct DailyUsage {
    limit: u64,
    state: Mutex<(NaiveDate, u64)>,
}

impl DailyUsage {
    fn new(limit: u64) -> Self {
        Self {
            limit,
            state: Mutex::new((Utc::now().date_naive(), 0)),
        }
    }

    fn with_today<T>(&self, f: impl FnOnce(&mut u64) -> T) -> T {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let today = Utc::now().date_naive();
        if state.0 != today {
            *state = (today, 0);
        }
        f(&mut state.1)
    }

    fn check(&self) -> Result<(), LlmError> {
        let limit = self.limit;
        self.with_today(|used| {
            if *used >= limit {
                Err(LlmError::BudgetExceeded {
                    scope: BudgetScope::Daily,
                    used: *used,
                    limit,
                })
            } else {
                Ok(())
            }
        })
    }

    fn add(&self, tokens: u64) {
        self.with_today(|used| *used = used.saturating_add(tokens));
    }

    fn used(&self) -> u64 {
        self.with_today(|used| *used)
    }
}

/// Claude Messages API の client
pub struct AnthropicClient {
    http: reqwest::Client,
    config: LlmConfig,
    permits: Semaphore,
    daily: DailyUsage,
}

impl std::fmt::Debug for AnthropicClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicClient")
            .field("base_url", &self.config.base_url)
            .field("max_concurrency", &self.config.max_concurrency)
            .field("api_key", &self.config.api_key)
            .finish_non_exhaustive()
    }
}

/// 1 回の送信の結果
enum Attempt {
    Done(Result<MessageResponse, LlmError>),
    Retry { error: LlmError, wait: Duration },
}

impl AnthropicClient {
    /// 設定から作る（env は読まない）
    pub fn new(config: LlmConfig) -> Result<Arc<Self>, LlmError> {
        if config.max_concurrency == 0 {
            return Err(LlmError::InvalidConfig(
                "max_concurrency must be at least 1".into(),
            ));
        }
        let http = reqwest::Client::builder()
            .connect_timeout(config.connect_timeout)
            .timeout(config.request_timeout)
            .build()
            .map_err(|e| LlmError::InvalidConfig(format!("failed to build HTTP client: {e}")))?;
        Ok(Arc::new(Self {
            http,
            permits: Semaphore::new(config.max_concurrency),
            daily: DailyUsage::new(config.max_tokens_per_day),
            config,
        }))
    }

    /// 今日（UTC）使った token 数
    pub fn daily_used(&self) -> u64 {
        self.daily.used()
    }

    fn url(&self) -> String {
        format!("{}/v1/messages", self.config.base_url.trim_end_matches('/'))
    }

    fn headers(&self) -> Result<HeaderMap, LlmError> {
        let mut headers = HeaderMap::new();
        let mut key = HeaderValue::from_str(self.config.api_key.expose())
            .map_err(|_| LlmError::InvalidConfig("API key is not a valid header value".into()))?;
        key.set_sensitive(true);
        headers.insert("x-api-key", key);
        headers.insert(
            "anthropic-version",
            HeaderValue::from_static(ANTHROPIC_VERSION),
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        Ok(headers)
    }

    fn backoff(&self, attempt: u32) -> Duration {
        self.config
            .retry_base_delay
            .saturating_mul(2u32.saturating_pow(attempt.saturating_sub(1)))
    }

    async fn attempt(&self, headers: &HeaderMap, body: &[u8], attempt: u32) -> Attempt {
        let permit = match self.permits.acquire().await {
            Ok(p) => p,
            Err(_) => return Attempt::Done(Err(LlmError::Other("client is closed".into()))),
        };
        let sent = self
            .http
            .post(self.url())
            .headers(headers.clone())
            .body(body.to_vec())
            .send()
            .await;
        let res = match sent {
            Ok(res) => res,
            Err(e) if e.is_connect() => {
                // 接続できていない = 送信していない。retry してよい
                return Attempt::Retry {
                    error: LlmError::Transport {
                        message: transport_message(&e),
                        attempts: attempt,
                    },
                    wait: self.backoff(attempt),
                };
            }
            Err(e) if e.is_timeout() => return Attempt::Done(Err(LlmError::Timeout)),
            Err(e) => {
                return Attempt::Done(Err(LlmError::Transport {
                    message: transport_message(&e),
                    attempts: attempt,
                }));
            }
        };
        let status = res.status().as_u16();
        let retry_after = parse_retry_after(res.headers());
        let header_request_id = res
            .headers()
            .get("request-id")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let bytes = match res.bytes().await {
            Ok(b) => b,
            Err(e) if e.is_timeout() => return Attempt::Done(Err(LlmError::Timeout)),
            Err(e) => {
                return Attempt::Done(Err(LlmError::Transport {
                    message: transport_message(&e),
                    attempts: attempt,
                }));
            }
        };
        drop(permit);

        if (200..300).contains(&status) {
            let parsed = serde_json::from_slice::<MessageResponse>(&bytes)
                .map_err(|e| LlmError::Decode(e.to_string()));
            if let Ok(resp) = &parsed {
                self.daily.add(resp.usage.total());
            }
            return Attempt::Done(parsed);
        }

        let body: Option<Value> = serde_json::from_slice(&bytes).ok();
        let err_obj = body.as_ref().and_then(|b| b.get("error"));
        let field = |k: &str| {
            err_obj
                .and_then(|e| e.get(k))
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        let spend_cap = err_obj
            .and_then(|e| e.pointer("/details/error_code"))
            .and_then(Value::as_str)
            == Some(SPEND_CAP_ERROR_CODE);
        let error = LlmError::Api {
            status,
            error_type: field("type").unwrap_or_default(),
            message: field("message").unwrap_or_else(|| {
                String::from_utf8_lossy(&bytes)
                    .chars()
                    .take(MAX_ERROR_BODY_CHARS)
                    .collect()
            }),
            request_id: body
                .as_ref()
                .and_then(|b| b.get("request_id"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .or(header_request_id),
        };
        let retryable = match status {
            429 => !spend_cap,
            s => s >= 500,
        };
        if !retryable {
            return Attempt::Done(Err(error));
        }
        let wait = match (status, retry_after) {
            (429, Some(wait)) if wait > self.config.max_retry_after => {
                return Attempt::Done(Err(error));
            }
            (429, Some(wait)) => wait,
            _ => self.backoff(attempt),
        };
        Attempt::Retry { error, wait }
    }
}

/// reqwest のエラー文（URL は含むが header は含まない）
fn transport_message(e: &reqwest::Error) -> String {
    let mut msg = e.to_string();
    let mut source = std::error::Error::source(e);
    while let Some(s) = source {
        msg.push_str(": ");
        msg.push_str(&s.to_string());
        source = s.source();
    }
    msg
}

/// `retry-after`（秒）。HTTP-date などは None（backoff に任せる）
fn parse_retry_after(headers: &HeaderMap) -> Option<Duration> {
    let raw = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();
    let secs: f64 = raw.parse().ok()?;
    (secs.is_finite() && secs >= 0.0).then(|| Duration::from_secs_f64(secs))
}

#[async_trait]
impl LlmClient for AnthropicClient {
    async fn create_message(&self, req: &MessageRequest) -> Result<MessageResponse, LlmError> {
        let body = serde_json::to_vec(req).map_err(|e| LlmError::Other(e.to_string()))?;
        let headers = self.headers()?;
        let mut attempt = 1;
        loop {
            self.daily.check()?;
            match self.attempt(&headers, &body, attempt).await {
                Attempt::Done(result) => return result,
                Attempt::Retry { error, wait } => {
                    if attempt > self.config.max_retries {
                        return Err(error);
                    }
                    tokio::time::sleep(wait).await;
                    attempt += 1;
                }
            }
        }
    }
}

/// env から作る process 共有の client（bin だけが呼ぶ）
///
/// 鍵が無ければ [`UnconfiguredLlm`]。一度作った client は process の中で使い回す。
pub fn shared_from_env() -> Result<Arc<dyn LlmClient>, LlmError> {
    static SHARED: OnceLock<Arc<dyn LlmClient>> = OnceLock::new();
    if let Some(client) = SHARED.get() {
        return Ok(client.clone());
    }
    let client: Arc<dyn LlmClient> = match LlmConfig::from_env()? {
        Some(config) => AnthropicClient::new(config)?,
        None => Arc::new(UnconfiguredLlm),
    };
    Ok(SHARED.get_or_init(|| client).clone())
}
