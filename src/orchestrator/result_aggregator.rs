//! Result Aggregator 実装

use crate::error::{Error, Result};
use crate::types::{ExecutionPattern, JsonMap, TaskResult, TaskStatus, is_truthy, ser_js_f64};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::HashSet;

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
    ///
    /// `dependencies`（タスクID → 依存タスクID。`OrchestrationPlan::dependencies` と同じ形）は
    /// hierarchical で親子を分けるのに使う。
    pub fn aggregate(
        &self,
        results: Vec<TaskResult>,
        pattern: ExecutionPattern,
        dependencies: Option<&[(String, Vec<String>)]>,
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
            ExecutionPattern::Hierarchical => Self::aggregate_hierarchical(&results, dependencies),
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

    /// 親と子を分けて木にする（TS #36）。
    ///
    /// - 親: `dependencies` があれば依存を持たない最初の結果、無ければ先頭の結果。
    /// - 子: 結果に含まれる最初の依存先（自分自身は除く）の下にネストする。
    ///   依存先が不明・不在のもの、循環して親へ辿り着けないものは親直下に置く。
    fn aggregate_hierarchical(
        results: &[TaskResult],
        dependencies: Option<&[(String, Vec<String>)]>,
    ) -> Value {
        let deps_of = |id: &str| -> Option<&[String]> {
            dependencies?
                .iter()
                .find(|(k, _)| k == id)
                .map(|(_, v)| v.as_slice())
        };
        let parent_idx = match dependencies {
            Some(_) => results.iter().position(|r| deps_of(&r.task_id).is_none()),
            None => (!results.is_empty()).then_some(0),
        };
        let Some(parent_idx) = parent_idx else {
            return json!({ "children": [] });
        };
        let parent = &results[parent_idx];
        let others: Vec<&TaskResult> = results
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != parent_idx)
            .map(|(_, r)| r)
            .collect();

        // 結果に含まれる最初の依存先。無ければ親。
        let parent_of = |r: &TaskResult| -> String {
            deps_of(&r.task_id)
                .and_then(|ds| {
                    ds.iter()
                        .find(|d| **d != r.task_id && results.iter().any(|x| &x.task_id == *d))
                })
                .cloned()
                .unwrap_or_else(|| parent.task_id.clone())
        };
        // 親へ辿り着けるか（循環なら false）
        let reaches_parent = |id: &str| -> bool {
            let mut seen = HashSet::new();
            let mut cur = Some(id.to_string());
            while let Some(c) = cur.take() {
                if c == parent.task_id {
                    return true;
                }
                if !seen.insert(c.clone()) {
                    return false;
                }
                cur = others.iter().find(|o| o.task_id == c).map(|o| parent_of(o));
            }
            false
        };

        // 各子の付け先（親直下なら None）
        let holders: Vec<Option<String>> = others
            .iter()
            .map(|r| {
                let pid = if reaches_parent(&r.task_id) {
                    parent_of(r)
                } else {
                    parent.task_id.clone()
                };
                (pid != parent.task_id).then_some(pid)
            })
            .collect();
        let children = hierarchy_nodes(None, &others, &holders, &mut vec![]);

        let mut m = JsonMap::new();
        if let Some(v) = &parent.result {
            m.insert("parent".into(), v.clone());
        }
        m.insert("parent_task_id".into(), json!(parent.task_id));
        m.insert("children".into(), Value::Array(children));
        Value::Object(m)
    }

    fn aggregate_conditional(results: &[TaskResult]) -> Value {
        json!({ "executed": results.iter().map(|r| entry(r, true)).collect::<Vec<_>>() })
    }
}

/// `holder` に付く子を `{ task_id, result, children }` の木にする（結果の順を保つ）。
/// `visiting` は念のための再帰ガード（`holders` は親へ辿り着ける鎖だけなので通常は循環しない）。
fn hierarchy_nodes(
    holder: Option<&str>,
    others: &[&TaskResult],
    holders: &[Option<String>],
    visiting: &mut Vec<String>,
) -> Vec<Value> {
    let mut out = vec![];
    for (r, h) in others.iter().zip(holders) {
        if h.as_deref() != holder || visiting.contains(&r.task_id) {
            continue;
        }
        let mut node = JsonMap::new();
        node.insert("task_id".into(), json!(r.task_id));
        if let Some(v) = &r.result {
            node.insert("result".into(), v.clone());
        }
        visiting.push(r.task_id.clone());
        let kids = hierarchy_nodes(Some(&r.task_id), others, holders, visiting);
        visiting.pop();
        node.insert("children".into(), Value::Array(kids));
        out.push(Value::Object(node));
    }
    out
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
                None,
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
            .aggregate(vec![], ExecutionPattern::Parallel, None)
            .unwrap();
        assert!(a.success);
        let s = serde_json::to_string(&a.metrics).unwrap();
        assert!(s.contains(r#""avg_confidence":0}"#), "{s}");
    }

    #[test]
    fn sequential_empty_is_error_not_panic() {
        assert!(
            ResultAggregator::new()
                .aggregate(vec![], ExecutionPattern::Sequential, None)
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
            .aggregate(rs.clone(), ExecutionPattern::Sequential, None)
            .unwrap()
            .aggregated;
        assert_eq!(
            s,
            json!({"chain": [{"task_id": "p", "result": {"x": 1}}, {"task_id": "c"}]})
        );
        let h = agg
            .aggregate(rs.clone(), ExecutionPattern::Hierarchical, None)
            .unwrap()
            .aggregated;
        // TS #36 以降: 子は children を持つノードになり、parent_task_id も返す
        assert_eq!(
            h,
            json!({
                "parent": {"x": 1},
                "parent_task_id": "p",
                "children": [{"task_id": "c", "children": []}]
            })
        );
        let c = agg
            .aggregate(rs, ExecutionPattern::Conditional, None)
            .unwrap()
            .aggregated;
        assert_eq!(
            c["executed"][1],
            json!({"task_id": "c", "status": "completed"})
        );
    }

    // --- TS src/orchestrator/result-aggregator.test.ts（#36）の移植 ---

    fn ok(id: &str, v: &str) -> TaskResult {
        r(id, TaskStatus::Completed, Some(json!(v)), None, 0)
    }

    fn deps(pairs: &[(&str, &[&str])]) -> Vec<(String, Vec<String>)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.iter().map(|d| d.to_string()).collect()))
            .collect()
    }

    fn hier(results: Vec<TaskResult>, d: Option<&[(String, Vec<String>)]>) -> Value {
        ResultAggregator::new()
            .aggregate(results, ExecutionPattern::Hierarchical, d)
            .unwrap()
            .aggregated
    }

    fn child_ids(v: &Value) -> Vec<String> {
        v["children"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["task_id"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn hierarchical_separates_parent_from_children_using_dependencies() {
        // 親が先頭でなくても依存を持たない結果が親になる
        let d = deps(&[("c1", &["p"]), ("c2", &["p"])]);
        let a = hier(vec![ok("c1", "C1"), ok("p", "P"), ok("c2", "C2")], Some(&d));
        assert_eq!(a["parent"], json!("P"));
        assert_eq!(a["parent_task_id"], json!("p"));
        assert_eq!(child_ids(&a), vec!["c1", "c2"]);
    }

    #[test]
    fn hierarchical_nests_grandchildren_under_their_dependency() {
        let d = deps(&[("c", &["p"]), ("g", &["c"])]);
        let a = hier(vec![ok("p", "P"), ok("c", "C"), ok("g", "G")], Some(&d));
        assert_eq!(child_ids(&a), vec!["c"]);
        assert_eq!(
            a["children"][0]["children"][0],
            json!({"task_id": "g", "result": "G", "children": []})
        );
    }

    #[test]
    fn hierarchical_falls_back_to_first_result_without_dependencies() {
        let a = hier(vec![ok("p", "P"), ok("c", "C")], None);
        assert_eq!(a["parent"], json!("P"));
        assert_eq!(child_ids(&a), vec!["c"]);
    }

    #[test]
    fn hierarchical_attaches_cyclic_children_to_parent_without_looping() {
        let d = deps(&[("a", &["b"]), ("b", &["a"])]);
        let a = hier(vec![ok("p", "P"), ok("a", "A"), ok("b", "B")], Some(&d));
        assert_eq!(a["parent"], json!("P"));
        assert_eq!(child_ids(&a), vec!["a", "b"]);
    }

    // --- 以下は TS の実装から読み取った境界（TS にテストは無い）---

    #[test]
    fn hierarchical_unknown_or_self_dependency_goes_under_parent() {
        // 結果に無い依存先・自分自身への依存は無視して親直下に置く
        let d = deps(&[("a", &["zzz"]), ("b", &["b"])]);
        let a = hier(vec![ok("p", "P"), ok("a", "A"), ok("b", "B")], Some(&d));
        assert_eq!(child_ids(&a), vec!["a", "b"]);
    }

    #[test]
    fn hierarchical_uses_first_dependency_present_in_results() {
        let d = deps(&[("c", &["p"]), ("g", &["missing", "c", "p"])]);
        let a = hier(vec![ok("p", "P"), ok("c", "C"), ok("g", "G")], Some(&d));
        assert_eq!(child_ids(&a), vec!["c"]);
        assert_eq!(a["children"][0]["children"][0]["task_id"], json!("g"));
    }

    #[test]
    fn hierarchical_all_children_means_no_parent() {
        // 全員が依存を持つ（親が見つからない）なら parent は undefined、children は空
        let d = deps(&[("a", &["b"]), ("b", &["a"])]);
        let a = hier(vec![ok("a", "A"), ok("b", "B")], Some(&d));
        assert_eq!(a, json!({"children": []}));
    }

    #[test]
    fn hierarchical_empty_results() {
        assert_eq!(hier(vec![], None), json!({"children": []}));
    }
}
