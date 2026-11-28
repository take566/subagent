//! ファイル操作とコマンド実行の閉じ込め
//!
//! - [`Workspace`]: `SUBAGENT_ROOTS` の中だけを読み書きする（必須。未設定なら拒否）
//! - [`run_command`]: `SUBAGENT_CODEGEN_ALLOWED_COMMANDS` にある program だけを shell なしで実行する
//!   （既定は無効。**allowlist は sandbox ではない**。詳しくは [`command`]）
//! - [`Sandbox`]: その 2 つを束ねたもの。ツールはこれを共有する

pub mod command;
pub mod workspace;

pub use command::{
    ALLOWED_COMMANDS_ENV, CommandOutput, CommandPolicy, DEFAULT_TIMEOUT_SECS, OUTPUT_LIMIT_CHARS,
    PASSTHROUGH_ENV, TIMEOUT_ENV, child_env, run_command, split_command_line,
};
pub use workspace::{
    ChangeKind, FileChange, MAX_FILE_BYTES_ENV, MAX_FILES_ENV, MayWrite, ROOTS_ENV, Workspace,
    WorkspaceLimits, validate_relative,
};

use crate::error::{Error, Result};
use std::path::PathBuf;
use std::sync::Arc;

/// Workspace（無ければその理由）とコマンドの policy
#[derive(Debug)]
pub struct Sandbox {
    workspace: std::result::Result<Arc<Workspace>, String>,
    policy: CommandPolicy,
}

fn reason(e: Error) -> String {
    match e {
        Error::Sandbox(m) => m,
        other => other.to_string(),
    }
}

impl Sandbox {
    pub fn new(workspace: Workspace, policy: CommandPolicy) -> Self {
        Sandbox {
            workspace: Ok(Arc::new(workspace)),
            policy,
        }
    }

    /// env（`SUBAGENT_ROOTS` ほか）から作る。roots が無ければファイル・コマンドのツールは拒否する
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        Sandbox {
            workspace: Workspace::from_lookup(&get).map(Arc::new).map_err(reason),
            policy: CommandPolicy::from_lookup(&get),
        }
    }

    pub fn from_env() -> Self {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// root を明示して作る（上限とコマンドの policy は env から）
    pub fn with_root(root: PathBuf) -> Self {
        let get = |k: &str| std::env::var(k).ok();
        let root = std::path::absolute(&root).unwrap_or(root);
        // 呼び出し側が明示した出力先なので、無ければ作る（以前の base_dir と同じ）
        let _ = std::fs::create_dir_all(&root);
        Sandbox {
            workspace: Workspace::with_limits(vec![root], WorkspaceLimits::from_lookup(get))
                .map(Arc::new)
                .map_err(reason),
            policy: CommandPolicy::from_lookup(get),
        }
    }

    /// Workspace。無ければ理由付きの [`Error::Sandbox`]
    pub fn workspace(&self) -> Result<Arc<Workspace>> {
        self.workspace.clone().map_err(Error::Sandbox)
    }

    /// タスクの始まり（[`Workspace::begin_task`]）。roots が無ければ何もしない
    pub fn begin_task(&self) {
        if let Ok(ws) = &self.workspace {
            ws.begin_task();
        }
    }

    /// 子プロセスに渡す env。PATH から workspace の中を指す entry を除く
    /// （workspace に置かれた同名の program を拾わないように）
    pub fn effective_env(&self) -> Result<Vec<(String, String)>> {
        let ws = self.workspace()?;
        Ok(self
            .policy
            .env()
            .iter()
            .filter_map(|(k, v)| {
                if k != "PATH" {
                    return Some((k.clone(), v.clone()));
                }
                let kept: Vec<_> = std::env::split_paths(v)
                    .filter(|p| !ws.contains(p))
                    .collect();
                let joined = std::env::join_paths(kept).ok()?;
                let joined = joined.to_string_lossy().into_owned();
                (!joined.is_empty()).then(|| (k.clone(), joined))
            })
            .collect())
    }

    pub fn policy(&self) -> &CommandPolicy {
        &self.policy
    }

    /// workspace 内の `cwd`（None は root）で program を実行する
    pub async fn run(
        &self,
        program: &str,
        args: &[String],
        cwd: Option<&str>,
    ) -> Result<CommandOutput> {
        self.policy.check_program(program)?;
        let ws = self.workspace()?;
        let dir = ws.resolve_dir(cwd)?;
        let policy = self.policy.clone().with_env(self.effective_env()?);
        run_command(&policy, &dir, program, args).await
    }

    /// コマンド行（shell の構文なし）を分けて実行する
    pub async fn run_line(&self, line: &str, cwd: Option<&str>) -> Result<CommandOutput> {
        let (program, args) = split_command_line(line)?;
        self.run(&program, &args, cwd).await
    }
}
