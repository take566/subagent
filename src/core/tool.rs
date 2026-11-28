//! ツールシステム

use crate::error::Result;
use crate::types::JsonMap;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// Subagent が呼び出すツール
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    /// パラメータ定義（`{ "<name>": { "type": ..., "required": ... } }`）
    fn parameters(&self) -> &JsonMap;
    async fn invoke(&self, params: JsonMap) -> Result<Value>;

    /// タスクの始まりに呼ばれる（sandbox のツールはタスクごとの書き込み件数を数え直す）
    fn begin_task(&self) {}
}

/// ツールの登録先。登録順を保持する（TS の `Map` と同じ）。
#[derive(Default, Clone)]
pub struct ToolRegistry {
    order: Vec<String>,
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// ツールを登録（同名は上書き、位置は最初の登録のまま）
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        let name = tool.name().to_string();
        if self.tools.insert(name.clone(), tool).is_none() {
            self.order.push(name);
        }
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn has(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    pub fn get_all(&self) -> Vec<Arc<dyn Tool>> {
        self.order.iter().map(|n| self.tools[n].clone()).collect()
    }

    pub fn get_names(&self) -> Vec<String> {
        self.order.clone()
    }
}

/// `{ name: { type, required } , ... }` 形式のパラメータ定義を作るヘルパー
pub fn param_defs(defs: &[(&str, &str, bool)]) -> JsonMap {
    defs.iter()
        .map(|(name, ty, required)| {
            (
                name.to_string(),
                serde_json::json!({ "type": ty, "required": required }),
            )
        })
        .collect()
}
