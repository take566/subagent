//! Result Aggregator 実装

use crate::error::{Error, Result};
use crate::types::{ExecutionPattern, JsonMap, TaskResult, TaskStatus, is_truthy, ser_js_f64};
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AggregatedMetrics {
    pub total_duration_ms: u64,
    pub success_count: u64,
    pub failure_count: u64,
    #[serde(serialize_with = "ser_js_f64")]
    pub avg_confidence: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AggregatedResult {
    pub success: bool,
    pub results: Vec<TaskResult>,
    pub aggregated: Value,
    pub metrics: AggregatedMetrics,
}

#[derive(Debug, Default, Clone)]
pub struct ResultAggregator;

/// `{ task_id, result }`（result が無ければキーごと省略。JS の undefined と同じ）
fn entry(r: &TaskResult, with_status: bool) -> Value {
    let mut m = JsonMap::new();
    m.insert("task_id".into(), json!(r.task_id));
    if let Some(v) = &r.result {
        m.insert("result".into(), v.clone());
    }
    if with_status {
        m.insert("status".into(), json!(r.status));
    }
    Value::Object(m)
}

impl ResultAggregator {
    pub fn new() -> Self {
        ResultAggregator
    }

    /// 結果を集約。sequential で結果が空のときはエラー（TS 版は TypeError）。
    pub fn aggregate(
        &self,
        results: Vec<TaskResult>,
        pattern: ExecutionPattern,
    ) -> Result<AggregatedResult> {
        let success_count = results
            .iter()
            .filter(|r| r.status == TaskStatus::Completed)
            .count() as u64;
        let failure_count = results.len() as u64 - success_count;
        let total_duration_ms = results
            .iter()
            .map(|r| r.metrics.as_ref().map(|m| m.duration_ms).unwrap_or(0))
            .sum();
        let confidences: Vec<f64> = results
            .iter()
            .map(|r| r.confidence.unwrap_or(0.0))
            .filter(|c| *c > 0.0)
            .collect();
        let avg_confidence = if confidences.is_empty() {
            0.0
        } else {
            confidences.iter().sum::<f64>() / confidences.len() as f64
        };

        let aggregated = match pattern {
            ExecutionPattern::Sequential => Self::aggregate_sequential(&results)?,
            ExecutionPattern::Parallel => Self::aggregate_parallel(&results),
            ExecutionPattern::Hierarchical => Self::aggregate_hierarchical(&results),
            ExecutionPattern::Conditional => Self::aggregate_conditional(&results),
        };

        Ok(AggregatedResult {
            success: success_count as usize == results.len(),
            metrics: AggregatedMetrics {
                total_duration_ms,
                success_count,
                failure_count,
                avg_confidence,
            },
            results,
            aggregated,
        })
    }

    /// 最後の結果と、各タスクの結果のチェーン
    fn aggregate_sequential(results: &[TaskResult]) -> Result<Value> {
        let last = results
            .last()
            .ok_or_else(|| Error::other("No results to aggregate for sequential execution"))?;
        let mut m = JsonMap::new();
        if let Some(v) = &last.result {
            m.insert("final_result".into(), v.clone());
        }
        m.insert(
            "chain".into(),
            Value::Array(results.iter().map(|r| entry(r, false)).collect()),
        );
        Ok(Value::Object(m))
    }

    fn aggregate_parallel(results: &[TaskResult]) -> Value {
        json!({
            "results": results.iter().map(|r| entry(r, true)).collect::<Vec<_>>(),
            "combined": Self::combine_results(results),
        })
    }

    /// `Object.assign({}, ...results)` 相当。文字列・配列は添字キーで展開される。
    fn combine_results(results: &[TaskResult]) -> Value {
        let mut combined = JsonMap::new();
        for v in results
            .iter()
            .filter_map(|r| r.result.as_ref())
            .filter(|v| is_truthy(v))
        {
            match v {
                Value::Object(m) => {
                    for (k, x) in m {
                        combined.insert(k.clone(), x.clone());
                    }
                }
                Value::Array(a) => {
                    for (i, x) in a.iter().enumerate() {
                        combined.insert(i.to_string(), x.clone());
                    }
                }
                Value::String(s) => {
                    for (i, u) in s.encode_utf16().enumerate() {
                        combined.insert(i.to_string(), json!(String::from_utf16_lossy(&[u])));
                    }
                }
                _ => {}
            }
        }
        Value::Object(combined)
    }

    fn aggregate_hierarchical(results: &[TaskResult]) -> Value {
        let mut m = JsonMap::new();
        if let Some(v) = results.first().and_then(|r| r.result.as_ref()) {
            m.insert("parent".into(), v.clone());
        }
        m.insert(
            "children".into(),
            Value::Array(results.iter().skip(1).map(|r| entry(r, false)).collect()),
        );
        Value::Object(m)
    }

    fn aggregate_conditional(results: &[TaskResult]) -> Value {
        json!({ "executed": results.iter().map(|r| entry(r, true)).collect::<Vec<_>>() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TaskMetrics;

    fn r(
        id: &str,
        status: TaskStatus,
        result: Option<Value>,
        conf: Option<f64>,
        ms: u64,
    ) -> TaskResult {
        TaskResult {
            task_id: id.into(),
            status,
            result,
            error: None,
            confidence: conf,
            metrics: Some(TaskMetrics {
                duration_ms: ms,
                ..Default::default()
            }),
        }
    }

    #[test]
    fn metrics_and_parallel_combine() {
        let a = ResultAggregator::new()
            .aggregate(
                vec![
                    r(
                        "1",
                        TaskStatus::Completed,
                        Some(json!({"a": 1, "b": 2})),
                        Some(0.5),
                        10,
                    ),
                    r(
                        "2",
                        TaskStatus::Failed,
                        Some(json!({"a": 3, "c": 4})),
                        Some(0.0),
                        5,
                    ),
                    r("3", TaskStatus::Completed, Some(json!("hi")), None, 0),
                ],
                ExecutionPattern::Parallel,
            )
            .unwrap();
        assert!(!a.success);
        assert_eq!(a.metrics.success_count, 2);
        assert_eq!(a.metrics.failure_count, 1);
        assert_eq!(a.metrics.total_duration_ms, 15);
        assert_eq!(a.metrics.avg_confidence, 0.5);
        // キー順は最初の挿入位置を保つ（JS の Object.assign と同じ）
        assert_eq!(
            serde_json::to_string(&a.aggregated["combined"]).unwrap(),
            r#"{"a":3,"b":2,"c":4,"0":"h","1":"i"}"#
        );
    }

    #[test]
    fn avg_confidence_zero_serializes_as_integer() {
        let a = ResultAggregator::new()
            .aggregate(vec![], ExecutionPattern::Parallel)
            .unwrap();
        assert!(a.success);
        let s = serde_json::to_string(&a.metrics).unwrap();
        assert!(s.contains(r#""avg_confidence":0}"#), "{s}");
    }

    #[test]
    fn sequential_empty_is_error_not_panic() {
        assert!(
            ResultAggregator::new()
                .aggregate(vec![], ExecutionPattern::Sequential)
                .is_err()
        );
    }

    #[test]
    fn sequential_hierarchical_conditional_shapes() {
        let rs = vec![
            r("p", TaskStatus::Completed, Some(json!({"x": 1})), None, 0),
            r("c", TaskStatus::Completed, None, None, 0),
        ];
        let agg = ResultAggregator::new();
        let s = agg
            .aggregate(rs.clone(), ExecutionPattern::Sequential)
            .unwrap()
            .aggregated;
        assert_eq!(
            s,
            json!({"chain": [{"task_id": "p", "result": {"x": 1}}, {"task_id": "c"}]})
        );
        let h = agg
            .aggregate(rs.clone(), ExecutionPattern::Hierarchical)
            .unwrap()
            .aggregated;
        assert_eq!(
            h,
            json!({"parent": {"x": 1}, "children": [{"task_id": "c"}]})
        );
        let c = agg
            .aggregate(rs, ExecutionPattern::Conditional)
            .unwrap()
            .aggregated;
        assert_eq!(
            c["executed"][1],
            json!({"task_id": "c", "status": "completed"})
        );
    }
}
