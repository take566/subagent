//! CodeGen の client tool: `text_editor_20250728`（name `str_replace_based_edit_tool`）を
//! [`Workspace`] で実行する [`EditorTools`]
//!
//! - command: `view`（ファイルは行番号付き、ディレクトリは 2 段まで）/ `create` / `str_replace` /
//!   `insert`。それ以外（`undo_edit` など）と引数の不足は `Err`（is_error の tool_result）
//! - 書いたファイルはこの値（1 回の typed tool の呼び出し）の中だけに記録する。
//!   [`Workspace::begin_task`] の全体の記録は使わない（並行する呼び出しが混ざらない、ruling 11）
//! - 書く前に deadline と cancel を見る。過ぎていれば書かない（F5）。書き込みは同期で、確認と
//!   書き込みの間に await は無いので、確認の後に future が捨てられて半端に書くことはない

use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use serde_json::Value;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::llm::{TEXT_EDITOR_NAME, ToolHandler};
use crate::sandbox::{FileChange, Workspace};

/// view で返す最大の文字数（超えた分は切って知らせる）
pub(crate) const MAX_VIEW_CHARS: usize = 100_000;
/// ディレクトリの view で並べる最大の件数
const MAX_LIST_ENTRIES: usize = 500;

pub(crate) struct EditorTools {
    ws: Arc<Workspace>,
    deadline: Instant,
    cancel: CancellationToken,
    log: Mutex<Vec<FileChange>>,
}

impl EditorTools {
    pub(crate) fn new(ws: Arc<Workspace>, deadline: Instant, cancel: CancellationToken) -> Self {
        Self {
            ws,
            deadline,
            cancel,
            log: Mutex::new(Vec::new()),
        }
    }

    /// この呼び出しで作った / 変えたファイル（最初に書いた順、root からの相対 path）
    pub(crate) fn changed(&self) -> Vec<String> {
        self.lock().iter().map(|c| c.path.clone()).collect()
    }

    fn lock(&self) -> MutexGuard<'_, Vec<FileChange>> {
        self.log.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 書いてよいか（cancel・deadline の後は書かない）
    fn may_write(&self) -> Result<(), String> {
        if self.cancel.is_cancelled() {
            return Err("the call was cancelled; nothing was written".into());
        }
        if Instant::now() >= self.deadline {
            return Err("deadline exceeded; nothing was written".into());
        }
        Ok(())
    }

    fn run(&self, input: &Value) -> Result<String, String> {
        let command = str_arg(input, "command")?;
        let path = str_arg(input, "path")?;
        match command {
            "view" => self.view(path, input),
            "create" => {
                let text = str_arg(input, "file_text")?;
                self.may_write()?;
                let kind = self
                    .ws
                    .create_tracked(&mut self.lock(), path, text)
                    .map_err(|e| e.to_string())?;
                Ok(match kind {
                    crate::sandbox::ChangeKind::Created => {
                        format!("File created successfully at: {path}")
                    }
                    crate::sandbox::ChangeKind::Modified => format!("File overwritten: {path}"),
                })
            }
            "str_replace" => {
                let old = str_arg(input, "old_str")?;
                let new = opt_str(input, "new_str")?.unwrap_or("");
                self.may_write()?;
                self.ws
                    .str_replace_tracked(&mut self.lock(), path, old, new)
                    .map_err(|e| e.to_string())?;
                Ok(format!("The file {path} has been edited."))
            }
            "insert" => {
                let line = input
                    .get("insert_line")
                    .and_then(Value::as_u64)
                    .ok_or("`insert_line` must be a non-negative integer")?;
                let line = usize::try_from(line).map_err(|_| "`insert_line` is too large")?;
                // 20250728 は insert_text。古い版の new_str も受ける
                let text = match opt_str(input, "insert_text")? {
                    Some(t) => t,
                    None => str_arg(input, "new_str")
                        .map_err(|_| "missing required field `insert_text`".to_string())?,
                };
                self.may_write()?;
                self.ws
                    .insert_tracked(&mut self.lock(), path, line, text)
                    .map_err(|e| e.to_string())?;
                Ok(format!("The file {path} has been edited."))
            }
            other => Err(format!(
                "unsupported command `{other}` (supported: view, create, str_replace, insert)"
            )),
        }
    }

    fn view(&self, path: &str, input: &Value) -> Result<String, String> {
        let range = match input.get("view_range") {
            None | Some(Value::Null) => None,
            Some(v) => Some(parse_range(v)?),
        };
        if is_root(path) {
            return self.list_dir(None, path);
        }
        let resolved = self.ws.resolve(path).map_err(|e| e.to_string())?;
        let meta = std::fs::symlink_metadata(&resolved).map_err(|e| format!("{path}: {e}"))?;
        if meta.is_dir() {
            return self.list_dir(Some(path), path);
        }
        let out = self.ws.view(path, range).map_err(|e| e.to_string())?;
        Ok(truncate(out, MAX_VIEW_CHARS))
    }

    /// ディレクトリの中身を 2 段まで並べる（`.` で始まる名前は除く。link は辿らない）
    fn list_dir(&self, rel: Option<&str>, shown: &str) -> Result<String, String> {
        let dir = self.ws.resolve_dir(rel).map_err(|e| e.to_string())?;
        let prefix = rel
            .map(|r| {
                r.split(['/', '\\'])
                    .filter(|p| !p.is_empty() && *p != ".")
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .unwrap_or_default();
        let mut out = Vec::new();
        let mut stack = vec![(dir, prefix, 0u32)];
        let mut truncated = false;
        while let Some((d, p, depth)) = stack.pop() {
            let mut entries: Vec<_> = std::fs::read_dir(&d)
                .map_err(|e| format!("{shown}: {e}"))?
                .filter_map(|e| e.ok())
                .collect();
            entries.sort_by_key(|e| e.file_name());
            for e in entries.into_iter().rev() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue;
                }
                if out.len() >= MAX_LIST_ENTRIES {
                    truncated = true;
                    break;
                }
                let rel = if p.is_empty() {
                    name
                } else {
                    format!("{p}/{name}")
                };
                let is_dir = std::fs::symlink_metadata(e.path()).is_ok_and(|m| m.is_dir());
                if is_dir {
                    out.push(format!("{rel}/"));
                    if depth + 1 < 2 {
                        stack.push((e.path(), rel, depth + 1));
                    }
                } else {
                    out.push(rel);
                }
            }
        }
        out.sort();
        let mut text = format!(
            "Files and directories in {shown} (up to 2 levels, hidden items excluded):\n{}",
            out.join("\n")
        );
        if truncated {
            text.push_str(&format!(
                "\n[listing truncated at {MAX_LIST_ENTRIES} entries]"
            ));
        }
        Ok(text)
    }
}

#[async_trait]
impl ToolHandler for EditorTools {
    async fn call(&self, name: &str, input: &Value) -> Result<String, String> {
        if name != TEXT_EDITOR_NAME {
            return Err(format!("unknown tool: {name}"));
        }
        self.run(input)
    }
}

fn str_arg<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    opt_str(input, key)?.ok_or_else(|| format!("missing required field `{key}`"))
}

fn opt_str<'a>(input: &'a Value, key: &str) -> Result<Option<&'a str>, String> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(format!("field `{key}` must be a string")),
    }
}

fn is_root(path: &str) -> bool {
    path.split(['/', '\\'])
        .all(|p| p.trim().is_empty() || p == ".")
}

/// `[start, end]`（1 始まり、end が -1 なら末尾まで）
fn parse_range(v: &Value) -> Result<(usize, Option<usize>), String> {
    let bad = || "`view_range` must be [start, end] (end -1 for the end of the file)".to_string();
    let a = v.as_array().filter(|a| a.len() == 2).ok_or_else(bad)?;
    let start = a[0]
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(bad)?;
    let end = match a[1].as_i64() {
        Some(-1) => None,
        Some(n) if n >= 0 => Some(usize::try_from(n).map_err(|_| bad())?),
        _ => return Err(bad()),
    };
    Ok((start, end))
}

/// `max` 文字で切る（切った時は印を付ける）
pub(crate) fn truncate(mut s: String, max: usize) -> String {
    if let Some((i, _)) = s.char_indices().nth(max) {
        let total = s.chars().count();
        s.truncate(i);
        s.push_str(&format!(
            "\n[truncated: showed {max} of {total} characters]"
        ));
    }
    s
}

/// テストのファイルらしい path か（`tests/` などの下、`test_*`、`*_test.*`、`*.test.*`、`*.spec.*`）
pub(crate) fn is_test_path(rel: &str) -> bool {
    let parts: Vec<&str> = rel.split('/').collect();
    let (file, dirs) = parts.split_last().unwrap_or((&"", &[]));
    let dir_hit = dirs.iter().any(|d| {
        matches!(
            d.to_ascii_lowercase().as_str(),
            "test" | "tests" | "__tests__" | "spec" | "specs" | "testdata"
        )
    });
    let lower = file.to_ascii_lowercase();
    let stem = lower.split('.').next().unwrap_or("");
    dir_hit
        || stem.starts_with("test_")
        || stem.ends_with("_test")
        || stem.ends_with("_spec")
        || lower.contains(".test.")
        || lower.contains(".spec.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_paths() {
        for p in [
            "tests/a.rs",
            "src/__tests__/x.js",
            "test_foo.py",
            "pkg/foo_test.go",
            "a.test.ts",
            "b.spec.js",
        ] {
            assert!(is_test_path(p), "{p}");
        }
        for p in ["src/lib.rs", "contest.rs", "latest/a.rs", "testing.md"] {
            assert!(!is_test_path(p), "{p}");
        }
    }

    #[test]
    fn ranges() {
        assert_eq!(parse_range(&serde_json::json!([1, -1])).unwrap(), (1, None));
        assert_eq!(
            parse_range(&serde_json::json!([2, 5])).unwrap(),
            (2, Some(5))
        );
        for bad in [
            serde_json::json!([1]),
            serde_json::json!("1-2"),
            serde_json::json!([1, -2]),
        ] {
            assert!(parse_range(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn truncate_marks_the_cut() {
        assert_eq!(truncate("abc".into(), 5), "abc");
        let t = truncate("あいうえお".into(), 2);
        assert!(t.starts_with("あい\n[truncated"), "{t}");
    }
}
