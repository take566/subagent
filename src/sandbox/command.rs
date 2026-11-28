//! run_command: allowlist にある program だけを shell を通さずに実行する
//!
//! - 既定は無効。`SUBAGENT_CODEGEN_ALLOWED_COMMANDS` に bare な program 名（`cargo,npm` など。
//!   path・`.exe` 以外の区切りを含む名前は無視）を並べたときだけ使える
//! - shell は使わない（`cmd /c` / `sh -c` なし）。引数はそのまま渡す
//! - cwd は workspace の中、stdin は null、timeout（`SUBAGENT_COMMAND_TIMEOUT_SECS`、既定 120 秒）で
//!   kill する。出力は stdout / stderr それぞれ約 3 万文字で切る
//! - env は空にしてから [`PASSTHROUGH_ENV`] だけを渡す（API 鍵や `NOTION_TOKEN` は子に渡らない）。
//!   PATH の相対 entry（`.` や空）は落とす。[`crate::sandbox::Sandbox::run`] は workspace の中を
//!   指す entry も落とす。Windows では `.exe` 以外の拡張子（`.bat` / `.cmd`）の program は使えない
//!
//! **これは sandbox ではない**。allowlist にある program（`cargo` / `npm` など）は build script や
//! package script を通じて任意のコードを実行できる。そうした program を許すのは、
//! コンテナ / WSL など捨ててよい環境の中だけにすること。timeout で kill するのは直接の子だけで、
//! 孫プロセスは残ることがある。

use crate::error::{Error, Result};
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};

pub const ALLOWED_COMMANDS_ENV: &str = "SUBAGENT_CODEGEN_ALLOWED_COMMANDS";
pub const TIMEOUT_ENV: &str = "SUBAGENT_COMMAND_TIMEOUT_SECS";
pub const DEFAULT_TIMEOUT_SECS: u64 = 120;
/// stdout / stderr それぞれの上限（文字数）
pub const OUTPUT_LIMIT_CHARS: usize = 30_000;
/// 子プロセスに渡す env（これ以外は渡さない）
pub const PASSTHROUGH_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "USERPROFILE",
    "TEMP",
    "TMP",
    "TMPDIR",
    "SystemRoot",
    // Windows の tool chain（cargo / rustup / git など）が使う、secret でない変数
    "APPDATA",
    "LOCALAPPDATA",
    "ProgramData",
    "SystemDrive",
    "PATHEXT",
    "ComSpec",
];

fn deny(msg: impl Into<String>) -> Error {
    Error::Sandbox(msg.into())
}

/// 実行結果
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandOutput {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    /// stdout か stderr を上限で切った
    pub truncated: bool,
}

/// どの program を、どの env・timeout で実行してよいか
#[derive(Debug, Clone)]
pub struct CommandPolicy {
    allowed: Vec<String>,
    timeout: Duration,
    env: Vec<(String, String)>,
    output_limit: usize,
}

/// bare な program 名か（path 区切り・ドライブ・空白・制御文字を含まない）。
/// Windows では `.exe` 以外の拡張子（`.bat` / `.cmd` など。cmd.exe を通って実行される）も拒否する
fn is_bare_program(p: &str) -> bool {
    let bare = !p.is_empty()
        && p != "."
        && p != ".."
        && !p.contains(['/', '\\', ':'])
        && !p.chars().any(|c| c.is_whitespace() || c.is_control());
    if cfg!(windows) {
        bare && p
            .rsplit_once('.')
            .is_none_or(|(_, ext)| ext.eq_ignore_ascii_case("exe"))
    } else {
        bare
    }
}

/// 比較用の名前（Windows は大文字小文字を区別せず、`.exe` を外す）
fn program_key(p: &str) -> String {
    if cfg!(windows) {
        let lower = p.to_ascii_lowercase();
        lower.strip_suffix(".exe").unwrap_or(&lower).to_string()
    } else {
        p.to_string()
    }
}

/// PATH から相対 entry（`.`、空、`bin` など）を落とす
fn sanitize_path(v: &str) -> Option<String> {
    let abs: Vec<_> = std::env::split_paths(v)
        .filter(|p| p.is_absolute())
        .collect();
    if abs.is_empty() {
        return None;
    }
    std::env::join_paths(abs)
        .ok()
        .map(|s| s.to_string_lossy().into_owned())
}

/// 子プロセスに渡す env（[`PASSTHROUGH_ENV`] にあるものだけ）
pub fn child_env(get: impl Fn(&str) -> Option<String>) -> Vec<(String, String)> {
    PASSTHROUGH_ENV
        .iter()
        .filter_map(|name| {
            let v = get(name)?;
            let v = if *name == "PATH" {
                sanitize_path(&v)?
            } else {
                v
            };
            Some((name.to_string(), v))
        })
        .collect()
}

fn process_env() -> Vec<(String, String)> {
    child_env(|k| std::env::var(k).ok())
}

impl CommandPolicy {
    /// 何も実行しない
    pub fn disabled() -> Self {
        CommandPolicy {
            allowed: vec![],
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
            env: process_env(),
            output_limit: OUTPUT_LIMIT_CHARS,
        }
    }

    /// `programs` を許可する（env は process のものから [`PASSTHROUGH_ENV`] だけ）
    pub fn allow(programs: &[&str]) -> Self {
        CommandPolicy {
            allowed: programs
                .iter()
                .filter(|p| is_bare_program(p))
                .map(|p| program_key(p))
                .collect(),
            ..Self::disabled()
        }
    }

    /// `SUBAGENT_CODEGEN_ALLOWED_COMMANDS`（`,` か空白区切り）と `SUBAGENT_COMMAND_TIMEOUT_SECS` を読む
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let allowed = get(ALLOWED_COMMANDS_ENV)
            .map(|v| {
                v.split(|c: char| c == ',' || c.is_whitespace())
                    .map(str::trim)
                    .filter(|p| is_bare_program(p))
                    .map(program_key)
                    .collect()
            })
            .unwrap_or_default();
        let timeout = get(TIMEOUT_ENV)
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(DEFAULT_TIMEOUT_SECS);
        CommandPolicy {
            allowed,
            timeout: Duration::from_secs(timeout),
            env: child_env(&get),
            output_limit: OUTPUT_LIMIT_CHARS,
        }
    }

    pub fn from_env() -> Self {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// 子プロセスに渡す env を差し替える
    pub fn with_env(mut self, env: Vec<(String, String)>) -> Self {
        self.env = env;
        self
    }

    pub fn with_output_limit(mut self, chars: usize) -> Self {
        self.output_limit = chars;
        self
    }

    pub fn is_enabled(&self) -> bool {
        !self.allowed.is_empty()
    }

    pub fn allowed(&self) -> &[String] {
        &self.allowed
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// 子プロセスに渡す env
    pub fn env(&self) -> &[(String, String)] {
        &self.env
    }

    /// 実行してよい program か
    pub fn check_program(&self, program: &str) -> Result<()> {
        if !self.is_enabled() {
            return Err(deny(format!(
                "running commands is disabled. Set {ALLOWED_COMMANDS_ENV} to a comma-separated \
                 list of bare program names (only inside a container / WSL: the allowlist is \
                 not a sandbox)"
            )));
        }
        if !is_bare_program(program) {
            return Err(deny(format!(
                "program must be a bare name found on PATH, not a path: {program:?}"
            )));
        }
        let key = program_key(program);
        if !self.allowed.contains(&key) {
            return Err(deny(format!(
                "{program:?} is not in {ALLOWED_COMMANDS_ENV} ({})",
                self.allowed.join(", ")
            )));
        }
        Ok(())
    }
}

/// コマンド行を program と引数に分ける（shell は使わない）。
///
/// 空白で区切り、`"..."` / `'...'` の中は 1 つの引数にする（中身はそのまま）。
/// 引用符の外の `| & ; < > ( ) $ `` ` `` と改行は shell の構文なので拒否する。
pub fn split_command_line(line: &str) -> Result<(String, Vec<String>)> {
    if line.contains(['\n', '\r']) {
        return Err(deny("newlines are not allowed in commands"));
    }
    let mut tokens: Vec<String> = vec![];
    let mut cur: Option<String> = None;
    let mut quote: Option<char> = None;
    for c in line.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.get_or_insert_with(String::new).push(c),
            None => match c {
                '"' | '\'' => {
                    quote = Some(c);
                    cur.get_or_insert_with(String::new);
                }
                '|' | '&' | ';' | '<' | '>' | '(' | ')' | '$' | '`' => {
                    return Err(deny(format!(
                        "shell syntax ({c:?}) is not supported: commands run without a shell. \
                         Pass one program and its arguments"
                    )));
                }
                c if c.is_whitespace() => {
                    if let Some(t) = cur.take() {
                        tokens.push(t);
                    }
                }
                c => cur.get_or_insert_with(String::new).push(c),
            },
        }
    }
    if quote.is_some() {
        return Err(Error::InvalidParameter(
            "unterminated quote in command".into(),
        ));
    }
    if let Some(t) = cur {
        tokens.push(t);
    }
    let mut it = tokens.into_iter();
    let Some(program) = it.next() else {
        return Err(Error::InvalidParameter("command is empty".into()));
    };
    Ok((program, it.collect()))
}

#[derive(Default)]
struct Capture {
    bytes: Vec<u8>,
    total: usize,
}

/// 上限まで貯め、残りは読み捨てる（子が pipe で詰まらないように）
async fn drain(mut r: impl AsyncRead + Unpin, cap: usize, out: Arc<Mutex<Capture>>) {
    let mut buf = [0u8; 8192];
    loop {
        match r.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let mut c = out.lock().unwrap_or_else(|e| e.into_inner());
                let room = cap.saturating_sub(c.bytes.len());
                c.bytes.extend_from_slice(&buf[..n.min(room)]);
                c.total += n;
            }
        }
    }
}

/// 文字数で切る。切ったら印を付けて true を返す
fn finish(c: &Mutex<Capture>, limit: usize) -> (String, bool) {
    let c = c.lock().unwrap_or_else(|e| e.into_inner());
    let text = String::from_utf8_lossy(&c.bytes);
    let mut truncated = c.total > c.bytes.len();
    let mut s: String = text.chars().take(limit).collect();
    if s.len() < text.len() {
        truncated = true;
    }
    if truncated {
        s.push_str(&format!(
            "\n...[truncated: output exceeded {limit} characters ({} bytes in total)]",
            c.total
        ));
    }
    (s, truncated)
}

/// `program args...` を `cwd` で実行する。
///
/// `cwd` は呼び出し側が workspace 内と確かめたディレクトリ（[`crate::sandbox::Sandbox::run`] を使う）
pub async fn run_command(
    policy: &CommandPolicy,
    cwd: &Path,
    program: &str,
    args: &[String],
) -> Result<CommandOutput> {
    policy.check_program(program)?;
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(policy.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .map_err(|e| Error::Other(format!("failed to start {program:?}: {e}")))?;

    let cap = policy.output_limit.saturating_mul(4).max(1);
    let out = Arc::new(Mutex::new(Capture::default()));
    let err = Arc::new(Mutex::new(Capture::default()));
    let readers = [
        child
            .stdout
            .take()
            .map(|s| tokio::spawn(drain(s, cap, out.clone()))),
        child
            .stderr
            .take()
            .map(|s| tokio::spawn(drain(s, cap, err.clone()))),
    ];

    let status = match tokio::time::timeout(policy.timeout, child.wait()).await {
        Ok(r) => Some(r.map_err(|e| Error::Other(format!("waiting for {program:?}: {e}")))?),
        Err(_) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            None
        }
    };
    // 孫プロセスが pipe を握っていても、少し待ったら読み終わりにする
    for r in readers.into_iter().flatten() {
        let abort = r.abort_handle();
        if tokio::time::timeout(Duration::from_secs(2), r)
            .await
            .is_err()
        {
            abort.abort();
        }
    }

    let (stdout, t1) = finish(&out, policy.output_limit);
    let (stderr, t2) = finish(&err, policy.output_limit);
    Ok(CommandOutput {
        success: status.is_some_and(|s| s.success()),
        exit_code: status.and_then(|s| s.code()),
        stdout,
        stderr,
        timed_out: status.is_none(),
        truncated: t1 || t2,
    })
}
