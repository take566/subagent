//! Planner 実装

use crate::tools::js_display;
use crate::types::{ConditionalRule, ExecutionPattern, OrchestrationPlan, Task};
use crate::utils::logger::Logger;
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct Planner {
    logger: Logger,
}

impl Default for Planner {
    fn default() -> Self {
        Self::new()
    }
}

fn has_parent(task: &Task) -> bool {
    task.parent_task().is_some_and(|p| !p.is_empty())
}

fn has_correlation(task: &Task) -> bool {
    task.correlation_id().is_some_and(|c| !c.is_empty())
}

impl Planner {
    pub fn new() -> Self {
        Planner {
            logger: Logger::new("planner"),
        }
    }

    /// 実行計画を作成（パターン未指定なら自動判定）
    pub fn create_plan(
        &self,
        tasks: Vec<Task>,
        pattern: Option<ExecutionPattern>,
    ) -> OrchestrationPlan {
        match pattern.unwrap_or_else(|| self.detect_pattern(&tasks)) {
            ExecutionPattern::Sequential => self.create_sequential_plan(tasks),
            ExecutionPattern::Parallel => OrchestrationPlan {
                pattern: ExecutionPattern::Parallel,
                tasks,
                dependencies: None,
                conditions: None,
            },
            ExecutionPattern::Hierarchical => self.create_hierarchical_plan(tasks),
            ExecutionPattern::Conditional => self.create_conditional_plan(tasks),
        }
    }

    /// 実行パターンを自動検出
    pub fn detect_pattern(&self, tasks: &[Task]) -> ExecutionPattern {
        if tasks.iter().any(|t| has_parent(t) || has_correlation(t)) {
            ExecutionPattern::Sequential
        } else if tasks.iter().any(|t| t.param_truthy("condition")) {
            ExecutionPattern::Conditional
        } else {
            ExecutionPattern::Parallel
        }
    }

    fn create_sequential_plan(&self, tasks: Vec<Task>) -> OrchestrationPlan {
        let ordered = self.topological_sort(&tasks);
        let dependencies = ordered
            .windows(2)
            .map(|w| (w[1].id.clone(), vec![w[0].id.clone()]))
            .collect();
        OrchestrationPlan {
            pattern: ExecutionPattern::Sequential,
            tasks: ordered,
            dependencies: Some(dependencies),
            conditions: None,
        }
    }

    fn create_hierarchical_plan(&self, tasks: Vec<Task>) -> OrchestrationPlan {
        let parent_id = tasks.iter().find(|t| !has_parent(t)).map(|t| t.id.clone());
        let dependencies = match &parent_id {
            Some(pid) => tasks
                .iter()
                .filter(|t| t.parent_task() == Some(pid.as_str()))
                .map(|t| (t.id.clone(), vec![pid.clone()]))
                .collect(),
            None => vec![],
        };
        OrchestrationPlan {
            pattern: ExecutionPattern::Hierarchical,
            tasks,
            dependencies: Some(dependencies),
            conditions: None,
        }
    }

    fn create_conditional_plan(&self, tasks: Vec<Task>) -> OrchestrationPlan {
        let conditions = tasks
            .iter()
            .filter(|t| t.param_truthy("condition"))
            .map(|t| ConditionalRule {
                condition: js_display(t.parameters.get("condition")),
                target_subagent: t
                    .parameters
                    .get("target_subagent")
                    .filter(|v| crate::types::is_truthy(v))
                    .map(|v| js_display(Some(v)))
                    .unwrap_or_else(|| "default".into()),
                action: t.action.clone(),
            })
            .collect();
        OrchestrationPlan {
            pattern: ExecutionPattern::Conditional,
            tasks,
            dependencies: None,
            conditions: Some(conditions),
        }
    }

    /// トポロジカルソート（依存先を先に並べる。循環は警告して無視）
    pub fn topological_sort(&self, tasks: &[Task]) -> Vec<Task> {
        let mut sorted = vec![];
        let mut visited = HashSet::new();
        let mut visiting = HashSet::new();
        for t in tasks {
            if !visited.contains(&t.id) {
                self.visit(t, tasks, &mut visited, &mut visiting, &mut sorted);
            }
        }
        sorted
    }

    fn visit(
        &self,
        task: &Task,
        all: &[Task],
        visited: &mut HashSet<String>,
        visiting: &mut HashSet<String>,
        sorted: &mut Vec<Task>,
    ) {
        if visiting.contains(&task.id) {
            self.logger.warn(
                &format!("Circular dependency detected for task {}", task.id),
                None,
            );
            return;
        }
        if visited.contains(&task.id) {
            return;
        }
        visiting.insert(task.id.clone());
        for dep in Self::get_dependencies(task, all) {
            if let Some(dep_task) = all.iter().find(|t| t.id == dep) {
                self.visit(dep_task, all, visited, visiting, sorted);
            }
        }
        visiting.remove(&task.id);
        visited.insert(task.id.clone());
        sorted.push(task.clone());
    }

    /// タスクの依存関係（parent_task と、同じ correlation_id を持つ他タスク）
    fn get_dependencies(task: &Task, all: &[Task]) -> Vec<String> {
        let mut deps: Vec<String> = vec![];
        if let Some(p) = task.parent_task().filter(|p| !p.is_empty()) {
            deps.push(p.into());
        }
        if let Some(cid) = task.correlation_id().filter(|c| !c.is_empty()) {
            let related: Vec<String> = all
                .iter()
                .filter(|t| {
                    t.correlation_id() == Some(cid) && t.id != task.id && !deps.contains(&t.id)
                })
                .map(|t| t.id.clone())
                .collect();
            deps.extend(related);
        }
        deps
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{TaskContext, TaskMetadata};
    use serde_json::json;

    fn task(id: &str, parent: Option<&str>) -> Task {
        let mut t = Task::new(id, "research", json!({}));
        if let Some(p) = parent {
            t.context = Some(TaskContext {
                parent_task: Some(p.into()),
                ..Default::default()
            });
        }
        t
    }

    fn ids(ts: &[Task]) -> Vec<&str> {
        ts.iter().map(|t| t.id.as_str()).collect()
    }

    #[test]
    fn detect() {
        let p = Planner::new();
        assert_eq!(
            p.detect_pattern(&[task("a", None)]),
            ExecutionPattern::Parallel
        );
        assert_eq!(
            p.detect_pattern(&[task("a", Some("b"))]),
            ExecutionPattern::Sequential
        );
        // 空文字の parent_task は falsy
        assert_eq!(
            p.detect_pattern(&[task("a", Some(""))]),
            ExecutionPattern::Parallel
        );
        let c = Task::new("c", "x", json!({"condition": "previous.success"}));
        assert_eq!(p.detect_pattern(&[c]), ExecutionPattern::Conditional);
        let c0 = Task::new("c", "x", json!({"condition": 0}));
        assert_eq!(p.detect_pattern(&[c0]), ExecutionPattern::Parallel);
        let mut m = task("m", None);
        m.metadata = Some(TaskMetadata {
            correlation_id: Some("x".into()),
            ..Default::default()
        });
        assert_eq!(p.detect_pattern(&[m]), ExecutionPattern::Sequential);
    }

    #[test]
    fn sequential_topo_sort() {
        let p = Planner::new();
        let plan = p.create_plan(
            vec![task("c", Some("b")), task("b", Some("a")), task("a", None)],
            None,
        );
        assert_eq!(plan.pattern, ExecutionPattern::Sequential);
        assert_eq!(ids(&plan.tasks), vec!["a", "b", "c"]);
        assert_eq!(plan.dependencies_of("c"), Some(&["b".to_string()][..]));
        assert_eq!(plan.dependencies_of("a"), None);
    }

    #[test]
    fn cycle_does_not_hang() {
        let p = Planner::new();
        let sorted = p.topological_sort(&[task("a", Some("b")), task("b", Some("a"))]);
        assert_eq!(ids(&sorted), vec!["b", "a"]);
    }

    #[test]
    fn hierarchical_and_conditional() {
        let p = Planner::new();
        let plan = p.create_plan(
            vec![
                task("root", None),
                task("c1", Some("root")),
                task("c2", Some("other")),
            ],
            Some(ExecutionPattern::Hierarchical),
        );
        assert_eq!(
            plan.dependencies,
            Some(vec![("c1".into(), vec!["root".into()])])
        );

        let plan = p.create_plan(
            vec![Task::new(
                "x",
                "review",
                json!({"condition": "previous.success"}),
            )],
            Some(ExecutionPattern::Conditional),
        );
        assert_eq!(
            plan.conditions,
            Some(vec![ConditionalRule {
                condition: "previous.success".into(),
                target_subagent: "default".into(),
                action: "review".into(),
            }])
        );
    }
}
