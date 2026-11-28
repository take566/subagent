//! リトライ機構実装

use crate::error::{Error, Result};
use crate::types::{Backoff, RetryPolicy};
use crate::utils::logger::Logger;
use std::future::Future;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct RetryManager {
    policy: RetryPolicy,
    logger: Logger,
}

impl RetryManager {
    pub fn new(policy: RetryPolicy, logger: Logger) -> Self {
        RetryManager { policy, logger }
    }

    /// バックオフ時間（ミリ秒）を計算
    fn calculate_backoff(&self, attempt: u32) -> u64 {
        let base = self.policy.initial_delay_ms.unwrap_or(1000);
        match self.policy.backoff {
            Backoff::Exponential => {
                base.saturating_mul(2u64.saturating_pow(attempt.saturating_sub(1)))
            }
            Backoff::Linear => base.saturating_mul(attempt as u64),
            Backoff::Fixed => base,
        }
    }

    /// リトライ可能か確認
    pub fn can_retry(&self, attempt: u32) -> bool {
        attempt < self.policy.max_attempts
    }

    /// 次のリトライまでの待機時間（ミリ秒）
    pub fn get_next_delay(&self, attempt: u32) -> u64 {
        if !self.can_retry(attempt) {
            return 0;
        }
        self.calculate_backoff(attempt)
    }

    /// リトライ付きで関数を実行
    pub async fn execute_with_retry<T, F, Fut>(&self, f: F) -> Result<T>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        self.execute_with_retry_cb(f, |_, _| {}).await
    }

    /// リトライ付きで関数を実行（失敗ごとに `on_retry(attempt, &error)` を呼ぶ）
    pub async fn execute_with_retry_cb<T, F, Fut, R>(&self, mut f: F, mut on_retry: R) -> Result<T>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T>>,
        R: FnMut(u32, &Error),
    {
        let max = self.policy.max_attempts;
        if max == 0 {
            // TS: ループに入らず `throw lastError || new Error('Retry failed')`
            return Err(Error::other("Retry failed"));
        }
        let mut attempt = 1;
        loop {
            match f().await {
                Ok(v) => return Ok(v),
                Err(err) => {
                    on_retry(attempt, &err);
                    // LLM 呼び出しの retry は client の中だけで行う。ここで重ねると、
                    // 送信後に失敗した request を送り直して二重に課金されうる
                    if matches!(err, Error::Llm(_)) {
                        return Err(err);
                    }
                    if !self.can_retry(attempt) {
                        self.logger
                            .warn(&format!("Max retries reached ({max})"), None);
                        return Err(err);
                    }
                    let delay = self.get_next_delay(attempt);
                    self.logger.info(
                        &format!("Retrying after {delay}ms (attempt {attempt}/{max})"),
                        None,
                    );
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                    attempt += 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn mgr(backoff: Backoff, max: u32, delay: u64) -> RetryManager {
        RetryManager::new(
            RetryPolicy {
                max_attempts: max,
                backoff,
                initial_delay_ms: Some(delay),
            },
            Logger::with_level(None, crate::utils::logger::LogLevel::Error),
        )
    }

    #[test]
    fn backoff_values() {
        let e = mgr(Backoff::Exponential, 5, 100);
        assert_eq!(
            (1..=4).map(|a| e.get_next_delay(a)).collect::<Vec<_>>(),
            vec![100, 200, 400, 800]
        );
        assert_eq!(e.get_next_delay(5), 0);
        let l = mgr(Backoff::Linear, 5, 100);
        assert_eq!(l.get_next_delay(3), 300);
        let f = mgr(Backoff::Fixed, 5, 100);
        assert_eq!(f.get_next_delay(3), 100);
    }

    #[test]
    fn default_delay_is_1000() {
        let m = RetryManager::new(
            RetryPolicy {
                max_attempts: 3,
                backoff: Backoff::Fixed,
                initial_delay_ms: None,
            },
            Logger::with_level(None, crate::utils::logger::LogLevel::Error),
        );
        assert_eq!(m.get_next_delay(1), 1000);
    }

    #[tokio::test]
    async fn retries_then_succeeds() {
        let m = mgr(Backoff::Fixed, 3, 1);
        let calls = AtomicU32::new(0);
        let r = m
            .execute_with_retry(|| async {
                if calls.fetch_add(1, Ordering::SeqCst) < 2 {
                    Err(Error::other("boom"))
                } else {
                    Ok(42)
                }
            })
            .await;
        assert_eq!(r.unwrap(), 42);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn llm_errors_are_never_retried() {
        // LLM の retry は client の中だけ。ここで包んでも二重に送らない（二重課金の防止）
        let m = mgr(Backoff::Fixed, 5, 1);
        let calls = AtomicU32::new(0);
        let mut seen = vec![];
        let r: Result<()> = m
            .execute_with_retry_cb(
                || async {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Err(Error::Llm(crate::llm::LlmError::Api {
                        status: 529,
                        error_type: "overloaded_error".into(),
                        message: "busy".into(),
                        request_id: None,
                    }))
                },
                |a, _| seen.push(a),
            )
            .await;
        assert_eq!(r.unwrap_err().name(), "LlmError");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(seen, vec![1]);
    }

    #[tokio::test]
    async fn gives_up_after_max_attempts() {
        let m = mgr(Backoff::Fixed, 2, 1);
        let calls = AtomicU32::new(0);
        let mut seen = vec![];
        let r: Result<()> = m
            .execute_with_retry_cb(
                || async {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Err(Error::other("boom"))
                },
                |a, _| seen.push(a),
            )
            .await;
        assert_eq!(r.unwrap_err().to_string(), "boom");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(seen, vec![1, 2]);
    }

    // 以下は TS 版 `tests/utils/retry.test.ts`（#42）で足りなかったケース（#65）

    #[test]
    fn linear_and_fixed_sequences() {
        let l = mgr(Backoff::Linear, 10, 100);
        assert_eq!(
            (1..=3).map(|a| l.get_next_delay(a)).collect::<Vec<_>>(),
            vec![100, 200, 300]
        );
        let f = mgr(Backoff::Fixed, 10, 100);
        assert_eq!(
            (1..=3).map(|a| f.get_next_delay(a)).collect::<Vec<_>>(),
            vec![100, 100, 100]
        );
        assert_eq!(mgr(Backoff::Fixed, 3, 100).get_next_delay(3), 0);
    }

    #[test]
    fn can_retry_only_below_max_attempts() {
        let m = mgr(Backoff::Fixed, 3, 100);
        assert!(m.can_retry(2));
        assert!(!m.can_retry(3));
    }

    #[tokio::test]
    async fn success_returns_immediately_without_on_retry() {
        let m = mgr(Backoff::Fixed, 3, 1);
        let calls = AtomicU32::new(0);
        let mut seen = vec![];
        let r = m
            .execute_with_retry_cb(
                || async {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok("ok")
                },
                |a, _| seen.push(a),
            )
            .await;
        assert_eq!(r.unwrap(), "ok");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(seen.is_empty());
    }

    #[tokio::test]
    async fn waits_backoff_between_attempts() {
        // exponential 20ms: 1 回目の後 20ms、2 回目の後 40ms 待つ
        let m = mgr(Backoff::Exponential, 3, 20);
        let calls = AtomicU32::new(0);
        let mut seen = vec![];
        let started = std::time::Instant::now();
        let r = m
            .execute_with_retry_cb(
                || async {
                    if calls.fetch_add(1, Ordering::SeqCst) < 2 {
                        Err(Error::other("e"))
                    } else {
                        Ok("done")
                    }
                },
                |a, _| seen.push(a),
            )
            .await;
        assert_eq!(r.unwrap(), "done");
        assert!(started.elapsed() >= Duration::from_millis(60));
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(seen, vec![1, 2]);
    }

    #[tokio::test]
    async fn returns_the_last_error() {
        let m = mgr(Backoff::Fixed, 3, 1);
        let calls = AtomicU32::new(0);
        let r: Result<()> = m
            .execute_with_retry(|| async {
                let n = calls.fetch_add(1, Ordering::SeqCst) + 1;
                Err(Error::other(format!("boom-{n}")))
            })
            .await;
        assert_eq!(r.unwrap_err().to_string(), "boom-3");
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    /// TS: `max_attempts` が 0 ならループに入らず fn を呼ばずに `Retry failed`
    #[tokio::test]
    async fn zero_max_attempts_never_calls_fn() {
        let m = mgr(Backoff::Fixed, 0, 1);
        let calls = AtomicU32::new(0);
        let r: Result<()> = m
            .execute_with_retry(|| async {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .await;
        assert_eq!(r.unwrap_err().to_string(), "Retry failed");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}
