//! Claude Messages API を raw HTTP（reqwest）で呼ぶ client
//!
//! - header: `x-api-key` / `anthropic-version: 2026-10-21` / `content-type: application/json`
//! - 1 process で 1 client を [`Arc`] で共有し、[`Semaphore`] で同時送信数を絞る
//! - retry は client の中だけ: 429（spend cap 以外）/ 500 / 529 / その他 5xx / 接続失敗。
//!   `retry-after` があれば待ち（`max_retry_after` を超える値や解釈できない値なら retry しない）、
//!   無ければ上限付き・jitter 付きの指数 backoff。
//!   400 / 401 / 402 / 403 / 404 / 413 と spend cap の 429 は retry しない。
//! - 送った後の失敗（timeout・通信断・body の読み出し失敗・2xx の decode 失敗）は課金された前提で
//!   retry せず、[`LlmError::PossiblyBilled`] として見積もりを 1 日の使用量に数える。
//! - process 単位の 1 日の token 上限: permit を取った後、mutex の中で
//!   `max_tokens + 入力の見積もり` を予約してから送り、応答の usage で精算する。

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{NaiveDate, Utc};
use rand::RngExt;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue, RETRY_AFTER};
use serde_json::Value;
use tokio::sync::Semaphore;
use tokio::time::Instant;

use super::client::{BudgetScope, LlmClient, LlmError, UnconfiguredLlm};
use super::config::{ANTHROPIC_VERSION, LlmConfig};
use super::types::{MessageRequest, MessageResponse};

/// spend cap の 429 で `error.details.error_code` に入る値
const SPEND_CAP_ERROR_CODE: &str = "enforced_spend_limit_reached";
/// 非 JSON の error body を message に入れる時の上限
const MAX_ERROR_BODY_CHARS: usize = 500;
/// 入力 token の見積もり: request body の byte 数をこれで割る（多めに見積もる）
const INPUT_BYTES_PER_TOKEN: u64 = 3;

#[derive(Debug)]
struct DayState {
    date: NaiveDate,
    used: u64,
    reserved: u64,
}

/// process 単位の 1 日の token 使用量（UTC の日付で区切る）
#[derive(Debug)]
struct DailyUsage {
    limit: u64,
    state: Mutex<DayState>,
}

impl DailyUsage {
    fn new(limit: u64) -> Self {
        Self {
            limit,
            state: Mutex::new(DayState {
                date: Utc::now().date_naive(),
                used: 0,
                reserved: 0,
            }),
        }
    }

    fn with_today<T>(&self, f: impl FnOnce(&mut DayState) -> T) -> T {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let today = Utc::now().date_naive();
        if state.date != today {
            *state = DayState {
                date: today,
                used: 0,
                reserved: 0,
            };
        }
        f(&mut state)
    }

    /// 使用量と予約の合計が上限に達していなければ `estimate` を予約する
    fn reserve(&self, estimate: u64) -> Result<Reservation<'_>, LlmError> {
        let limit = self.limit;
        self.with_today(|s| {
            let committed = s.used.saturating_add(s.reserved);
            if committed >= limit {
                return Err(LlmError::BudgetExceeded {
                    scope: BudgetScope::Daily,
                    used: committed,
                    limit,
                });
            }
            s.reserved = s.reserved.saturating_add(estimate);
            Ok(())
        })?;
        Ok(Reservation {
            daily: self,
            estimate,
            settled: false,
        })
    }

    fn settle(&self, estimate: u64, actual: u64) {
        self.with_today(|s| {
            s.reserved = s.reserved.saturating_sub(estimate);
            s.used = s.used.saturating_add(actual);
        });
    }

    fn used(&self) -> u64 {
        self.with_today(|s| s.used)
    }
}

/// 1 回の送信の予約。精算せずに drop された（future が途中で捨てられた）時は課金された前提で数える
struct Reservation<'a> {
    daily: &'a DailyUsage,
    estimate: u64,
    settled: bool,
}

impl Reservation<'_> {
    fn settle(mut self, actual: u64) {
        self.settled = true;
        self.daily.settle(self.estimate, actual);
    }

    /// 送った後に失敗した: 見積もりを数え、PossiblyBilled を返す
    fn billed(self, reason: impl Into<String>) -> LlmError {
        let estimated_tokens = self.estimate;
        self.settle(estimated_tokens);
        LlmError::PossiblyBilled {
            reason: reason.into(),
            estimated_tokens,
        }
    }
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        if !self.settled {
            self.daily.settle(self.estimate, self.estimate);
        }
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

/// `retry-after` header
#[derive(Debug, PartialEq)]
enum RetryAfter {
    Absent,
    Seconds(Duration),
    /// HTTP-date など、秒数として読めない値
    Unparseable,
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

    /// 今日（UTC）使った token 数（課金された可能性がある分の見積もりを含む）
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

    /// 上限付きの指数 backoff に jitter を付ける（`d/2 ..= d`、d ≤ max_backoff）
    fn backoff(&self, attempt: u32) -> Duration {
        let exp = self
            .config
            .retry_base_delay
            .saturating_mul(2u32.saturating_pow(attempt.saturating_sub(1)));
        let capped = exp.min(self.config.max_backoff);
        let half = capped / 2;
        let half_micros = u64::try_from(half.as_micros()).unwrap_or(u64::MAX);
        half + Duration::from_micros(rand::rng().random_range(0..=half_micros))
    }

    /// 1 回送る。`timeout` はこの request 全体の timeout
    async fn attempt(
        &self,
        headers: &HeaderMap,
        body: &[u8],
        estimate: u64,
        timeout: Duration,
        attempt: u32,
    ) -> Attempt {
        let permit = match self.permits.acquire().await {
            Ok(p) => p,
            Err(_) => return Attempt::Done(Err(LlmError::Other("client is closed".into()))),
        };
        // permit を取った後に予約するので、待っていた request も上限を見直す
        let reservation = match self.daily.reserve(estimate) {
            Ok(r) => r,
            Err(e) => return Attempt::Done(Err(e)),
        };
        let sent = self
            .http
            .post(self.url())
            .headers(headers.clone())
            .timeout(timeout)
            .body(body.to_vec())
            .send()
            .await;
        let res = match sent {
            Ok(res) => res,
            Err(e) if e.is_connect() => {
                // 接続できていない = 送っていない。課金されないので予約を戻して retry
                reservation.settle(0);
                return Attempt::Retry {
                    error: LlmError::Transport {
                        message: transport_message(&e),
                        attempts: attempt,
                    },
                    wait: self.backoff(attempt),
                };
            }
            Err(e) if e.is_timeout() => {
                return Attempt::Done(Err(
                    reservation.billed("timed out after the request was sent")
                ));
            }
            Err(e) => {
                return Attempt::Done(Err(reservation.billed(format!(
                    "connection failed after the request was sent: {}",
                    transport_message(&e)
                ))));
            }
        };
        let status = res.status().as_u16();
        let retry_after = parse_retry_after(res.headers());
        let header_request_id = res
            .headers()
            .get("request-id")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let success = (200..300).contains(&status);
        let bytes = match res.bytes().await {
            Ok(b) => b,
            Err(e) if success || e.is_timeout() => {
                return Attempt::Done(Err(reservation.billed(format!(
                    "response body could not be read: {}",
                    transport_message(&e)
                ))));
            }
            Err(e) => {
                // error 応答（課金されない）の body が読めなかった
                reservation.settle(0);
                return Attempt::Done(Err(LlmError::Api {
                    status,
                    error_type: String::new(),
                    message: format!("error body could not be read: {}", transport_message(&e)),
                    request_id: header_request_id,
                }));
            }
        };
        drop(permit);

        if success {
            return Attempt::Done(match serde_json::from_slice::<MessageResponse>(&bytes) {
                Ok(resp) => {
                    reservation.settle(resp.usage.total());
                    Ok(resp)
                }
                Err(e) => {
                    Err(reservation.billed(format!("2xx response could not be decoded: {e}")))
                }
            });
        }
        // error 応答は課金されない
        reservation.settle(0);

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
        let wait = match retry_after {
            RetryAfter::Unparseable => return Attempt::Done(Err(error)),
            RetryAfter::Seconds(wait) if wait > self.config.max_retry_after => {
                return Attempt::Done(Err(error));
            }
            RetryAfter::Seconds(wait) => wait,
            RetryAfter::Absent => self.backoff(attempt),
        };
        Attempt::Retry { error, wait }
    }

    async fn send(
        &self,
        req: &MessageRequest,
        deadline: Option<Instant>,
    ) -> Result<MessageResponse, LlmError> {
        let body = serde_json::to_vec(req).map_err(|e| LlmError::Other(e.to_string()))?;
        let estimate =
            u64::from(req.max_tokens).saturating_add(body.len() as u64 / INPUT_BYTES_PER_TOKEN);
        let headers = self.headers()?;
        let mut attempt = 1;
        loop {
            let timeout = match deadline {
                None => self.config.request_timeout,
                Some(d) => {
                    let left = d.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Err(LlmError::DeadlineExceeded);
                    }
                    left.min(self.config.request_timeout)
                }
            };
            match self
                .attempt(&headers, &body, estimate, timeout, attempt)
                .await
            {
                Attempt::Done(result) => return result,
                Attempt::Retry { error, wait } => {
                    if attempt > self.config.max_retries
                        || deadline.is_some_and(|d| Instant::now() + wait >= d)
                    {
                        return Err(error);
                    }
                    tokio::time::sleep(wait).await;
                    attempt += 1;
                }
            }
        }
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

/// `retry-after`（秒）
fn parse_retry_after(headers: &HeaderMap) -> RetryAfter {
    let Some(raw) = headers.get(RETRY_AFTER) else {
        return RetryAfter::Absent;
    };
    match raw.to_str().ok().and_then(|s| s.trim().parse::<f64>().ok()) {
        Some(secs) if secs.is_finite() && secs >= 0.0 => {
            RetryAfter::Seconds(Duration::from_secs_f64(secs))
        }
        _ => RetryAfter::Unparseable,
    }
}

#[async_trait]
impl LlmClient for AnthropicClient {
    async fn create_message(&self, req: &MessageRequest) -> Result<MessageResponse, LlmError> {
        self.send(req, None).await
    }

    async fn create_message_until(
        &self,
        req: &MessageRequest,
        deadline: Option<Instant>,
    ) -> Result<MessageResponse, LlmError> {
        self.send(req, deadline).await
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_is_capped_and_jittered() {
        let mut config = LlmConfig::new("k");
        config.retry_base_delay = Duration::from_millis(100);
        config.max_backoff = Duration::from_millis(400);
        let c = AnthropicClient::new(config).unwrap();
        for attempt in 1..=20u32 {
            let wait = c.backoff(attempt);
            let full = Duration::from_millis(100 * 2u64.pow(attempt.min(10) - 1))
                .min(Duration::from_millis(400));
            assert!(wait >= full / 2 && wait <= full, "{attempt}: {wait:?}");
        }
    }

    #[test]
    fn retry_after_parsing() {
        let h = |v: &str| {
            let mut m = HeaderMap::new();
            m.insert(RETRY_AFTER, HeaderValue::from_str(v).unwrap());
            parse_retry_after(&m)
        };
        assert_eq!(parse_retry_after(&HeaderMap::new()), RetryAfter::Absent);
        assert_eq!(h("2"), RetryAfter::Seconds(Duration::from_secs(2)));
        assert_eq!(h("0.5"), RetryAfter::Seconds(Duration::from_millis(500)));
        assert_eq!(h("Wed, 21 Oct 2026 07:28:00 GMT"), RetryAfter::Unparseable);
        assert_eq!(h("-1"), RetryAfter::Unparseable);
    }

    #[test]
    fn dropped_reservation_is_counted_as_billed() {
        let daily = DailyUsage::new(1000);
        drop(daily.reserve(70).unwrap());
        assert_eq!(daily.used(), 70);
        daily.reserve(30).unwrap().settle(5);
        assert_eq!(daily.used(), 75);
    }
}
