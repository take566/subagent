//! typed tool の 1 回の呼び出しに付く文脈（[`CallCtx`]）と進捗の送り先（[`ProgressSink`]）
//!
//! MCP server（[`crate::mcp`]）が request を受けた時に作り、[`crate::Subagent::call_typed`] に渡す。
//!
//! - `deadline`: request を受けた時刻 + `SUBAGENT_DEADLINE_SECS`（behavior.timeout が短ければそちら）。
//!   server 側でも打ち切るが、agent は [`crate::llm::RunnerLimits::deadline`] に渡して
//!   LLM 呼び出しと tool をその前に止める。
//! - `progress`: request に `_meta.progressToken` が無ければ何も送らない。
//! - `cancel`: client の `notifications/cancelled` で cancel される。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use tokio::sync::Mutex;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::llm::{MessageResponse, RunObserver};

/// 進捗を実際に送る先（MCP では `notifications/progress`）
#[async_trait]
pub trait ProgressNotifier: Send + Sync {
    /// `progress` は 1 つの sink の中で必ず増え続ける。`total` は最後の通知だけに付く
    async fn notify(&self, progress: f64, total: Option<f64>, message: Option<String>);
}

struct SinkInner {
    notifier: Arc<dyn ProgressNotifier>,
    /// finish / close が始まったら立つ（lock を待たずに以後の report を止める）
    closing: AtomicBool,
    /// 最後に振った番号。番号を振ってから送り終えるまで持つ
    last: Mutex<u64>,
}

/// 進捗の送り先。clone は同じ counter を共有する。
///
/// - [`ProgressSink::none`]（token が無い request）は何も送らない
/// - 送る値は 1, 2, 3, ... と増え続ける（並行に呼んでも順に送る）
/// - [`ProgressSink::finish`] / [`ProgressSink::close`] の後は何も送らない（応答の後に通知が届かない）
#[derive(Clone, Default)]
pub struct ProgressSink(Option<Arc<SinkInner>>);

impl std::fmt::Debug for ProgressSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgressSink")
            .field("enabled", &self.is_enabled())
            .finish()
    }
}

impl ProgressSink {
    /// 何も送らない sink
    pub fn none() -> Self {
        Self(None)
    }

    pub fn new(notifier: Arc<dyn ProgressNotifier>) -> Self {
        Self(Some(Arc::new(SinkInner {
            notifier,
            closing: AtomicBool::new(false),
            last: Mutex::new(0),
        })))
    }

    /// 送る先があるか（token 付きの request か）
    pub fn is_enabled(&self) -> bool {
        self.0.is_some()
    }

    /// 進捗を 1 つ送る（終了後と sink が無い時は何もしない）
    pub async fn report(&self, message: impl Into<String>) {
        let Some(inner) = &self.0 else { return };
        if inner.closing.load(Ordering::SeqCst) {
            return;
        }
        // 番号を振ってから送るまで lock を持つ（送る順と番号の順を揃える）
        let mut last = inner.last.lock().await;
        if inner.closing.load(Ordering::SeqCst) {
            return;
        }
        *last += 1;
        let progress = *last as f64;
        inner
            .notifier
            .notify(progress, None, Some(message.into()))
            .await;
    }

    /// 最後の進捗（`total` 付き）を送り、以後の [`Self::report`] を捨てる。応答を返す前に server が呼ぶ。
    /// 先に閉じるので、途中で future を捨てても（timeout）以後の進捗は送らない
    pub async fn finish(&self, message: impl Into<String>) {
        let Some(inner) = &self.0 else { return };
        if inner.closing.swap(true, Ordering::SeqCst) {
            return;
        }
        let mut last = inner.last.lock().await;
        *last += 1;
        let progress = *last as f64;
        inner
            .notifier
            .notify(progress, Some(progress), Some(message.into()))
            .await;
    }

    /// 何も送らずに閉じる（client が取り消した時）。送っている途中の通知があれば終わるまで待つ
    pub async fn close(&self) {
        let Some(inner) = &self.0 else { return };
        inner.closing.store(true, Ordering::SeqCst);
        drop(inner.last.lock().await);
    }
}

/// runner の往復 1 回ごとに進捗を送る
#[async_trait]
impl RunObserver for ProgressSink {
    async fn on_iteration(&self, iteration: u32, _response: &MessageResponse) {
        self.report(format!("iteration {iteration}")).await;
    }
}

/// typed tool の 1 回の呼び出しの文脈
#[derive(Debug, Clone)]
pub struct CallCtx {
    /// これを過ぎたら server が打ち切る（agent は runner の deadline に渡す）
    pub deadline: Instant,
    pub progress: ProgressSink,
    /// client が取り消すと cancel される
    pub cancel: CancellationToken,
}

impl CallCtx {
    /// deadline だけを決めた文脈（進捗なし、取り消しなし）。テストや MCP 以外から呼ぶ用
    pub fn with_deadline(deadline: Instant) -> Self {
        Self {
            deadline,
            progress: ProgressSink::none(),
            cancel: CancellationToken::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    type Sent = Vec<(f64, Option<f64>, Option<String>)>;

    #[derive(Default)]
    struct Rec(StdMutex<Sent>);

    #[async_trait]
    impl ProgressNotifier for Rec {
        async fn notify(&self, progress: f64, total: Option<f64>, message: Option<String>) {
            tokio::task::yield_now().await;
            self.0.lock().unwrap().push((progress, total, message));
        }
    }

    #[tokio::test]
    async fn close_sends_nothing_and_stops_later_reports() {
        let rec = Arc::new(Rec::default());
        let s = ProgressSink::new(rec.clone());
        s.report("a").await;
        s.close().await;
        s.report("b").await;
        s.finish("c").await;
        let got = rec.0.lock().unwrap().clone();
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].2.as_deref(), Some("a"));
    }

    /// finish の送信を途中で捨てても（timeout）、以後の report は送らない
    #[tokio::test(start_paused = true)]
    async fn abandoned_finish_still_closes() {
        struct Slow(StdMutex<u32>);
        #[async_trait]
        impl ProgressNotifier for Slow {
            async fn notify(&self, _: f64, _: Option<f64>, _: Option<String>) {
                tokio::time::sleep(std::time::Duration::from_secs(100)).await;
                *self.0.lock().unwrap() += 1;
            }
        }
        let slow = Arc::new(Slow(StdMutex::new(0)));
        let s = ProgressSink::new(slow.clone());
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), s.finish("x")).await;
        s.report("late").await;
        assert_eq!(*slow.0.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn none_sends_nothing() {
        let s = ProgressSink::none();
        assert!(!s.is_enabled());
        s.report("x").await;
        s.finish("y").await;
    }

    #[tokio::test]
    async fn progress_strictly_increases_and_stops_after_finish() {
        let rec = Arc::new(Rec::default());
        let s = ProgressSink::new(rec.clone());
        let tasks: Vec<_> = (0..20)
            .map(|i| {
                let s = s.clone();
                tokio::spawn(async move { s.report(format!("r{i}")).await })
            })
            .collect();
        for t in tasks {
            t.await.unwrap();
        }
        s.finish("done").await;
        s.report("late").await;
        s.finish("late").await;
        let got = rec.0.lock().unwrap().clone();
        assert_eq!(got.len(), 21);
        assert!(got.windows(2).all(|w| w[0].0 < w[1].0), "{got:?}");
        let last = got.last().unwrap();
        assert_eq!(last.1, Some(last.0));
        assert_eq!(last.2.as_deref(), Some("done"));
        assert!(got[..20].iter().all(|g| g.1.is_none()));
    }
}
