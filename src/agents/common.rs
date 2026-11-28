//! 4 エージェントの typed tool で共有する小さな helper

use rmcp::model::Tool as McpTool;
use serde_json::Value;

use crate::types::JsonMap;

/// typed tool の定義。`schema` は `additionalProperties: false` の object schema
pub(crate) fn typed_tool(name: &'static str, description: &'static str, schema: Value) -> McpTool {
    let Value::Object(schema) = schema else {
        unreachable!("typed tool schema must be an object")
    };
    McpTool::new(name, description, schema)
}

/// `args` の `from` を `params` の `to` に写す（無ければ何もしない）
pub(crate) fn copy_arg(args: &Value, from: &str, params: &mut JsonMap, to: &str) {
    if let Some(v) = args.get(from) {
        params.insert(to.into(), v.clone());
    }
}
