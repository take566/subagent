//! Workspace: ファイル操作を `SUBAGENT_ROOTS` の中に閉じ込める
//!
//! - roots は必須。未設定なら読み書き系のツールは拒否する（cwd を既定にしない）
//! - path は root からの相対 path だけを受け付ける。絶対 path・`..`・ドライブ指定（`C:`）・
//!   `\\?\` / UNC・`:`（NTFS の代替データストリーム）・Windows の予約名（`CON` / `NUL` / `COM1` など。
//!   拡張子付きも）・末尾のドット / 空白は、OS に関係なく拒否する
//! - 最寄りの既存祖先を canonicalize して root の中にあるかを確かめ、さらに **open した handle の
//!   実 path** を確かめる（Windows は `GetFinalPathNameByHandleW`、Linux は `/proc/self/fd`、
//!   macOS は `fcntl(F_GETPATH)`）。resolve と open の間に symlink / junction へ差し替えられても
//!   root の外は読み書きしない
//! - 1 ファイルのサイズと、書き込んだファイルの件数に上限を置く
//! - 操作は 1 つずつ（内部の lock で直列化する）
//!
//! 限界: open 後の検証で root の外と分かった場合でも、`create` が空のファイルを作ってしまった
//! 後のことがある（中身は書かない・既存ファイルは空にしない）。リンク数が 2 以上のファイル
//! （ハードリンク）には書かない。

use crate::error::{Error, Result};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

/// workspace の root（必須）。複数なら OS の path 区切り（Windows は `;`、それ以外は `:`）で並べる
pub const ROOTS_ENV: &str = "SUBAGENT_ROOTS";
/// 1 ファイルの上限（byte）
pub const MAX_FILE_BYTES_ENV: &str = "SUBAGENT_MAX_FILE_BYTES";
/// 書き込むファイルの件数の上限（workspace ごと、つまり process ごと）
pub const MAX_FILES_ENV: &str = "SUBAGENT_MAX_FILES";

pub const DEFAULT_MAX_FILE_BYTES: u64 = 1024 * 1024;
pub const DEFAULT_MAX_FILES: usize = 200;

fn deny(msg: impl Into<String>) -> Error {
    Error::Sandbox(msg.into())
}

/// roots 未設定のときのメッセージ
pub fn roots_unset_message() -> String {
    format!(
        "{ROOTS_ENV} is not set: file and command tools are disabled. \
         Set {ROOTS_ENV} to the absolute path of the workspace directory"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Created,
    Modified,
}

/// workspace で作った / 変えたファイル（`path` は root からの相対 path、区切りは `/`）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    pub kind: ChangeKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceLimits {
    pub max_file_bytes: u64,
    pub max_files: usize,
}

impl Default for WorkspaceLimits {
    fn default() -> Self {
        WorkspaceLimits {
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            max_files: DEFAULT_MAX_FILES,
        }
    }
}

impl WorkspaceLimits {
    /// `SUBAGENT_MAX_FILE_BYTES` / `SUBAGENT_MAX_FILES` を読む（正の整数でなければ既定値）
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let d = Self::default();
        WorkspaceLimits {
            max_file_bytes: get(MAX_FILE_BYTES_ENV)
                .and_then(|v| v.trim().parse().ok())
                .filter(|n| *n > 0)
                .unwrap_or(d.max_file_bytes),
            max_files: get(MAX_FILES_ENV)
                .and_then(|v| v.trim().parse().ok())
                .filter(|n| *n > 0)
                .unwrap_or(d.max_files),
        }
    }
}

#[derive(Debug)]
struct Root {
    /// 設定どおりの path（結果の表示用）
    display: PathBuf,
    /// canonicalize した path（検証用）
    canonical: PathBuf,
}

#[derive(Debug)]
pub struct Workspace {
    roots: Vec<Root>,
    limits: WorkspaceLimits,
    /// 書き込んだファイル。lock は操作の直列化も兼ねる
    changes: Mutex<Vec<FileChange>>,
}

/// Windows の予約デバイス名か（拡張子・末尾の空白を除いた部分で判定。大文字小文字は区別しない）
fn is_reserved_name(part: &str) -> bool {
    let stem = part.split('.').next().unwrap_or("").trim_end_matches(' ');
    let upper = stem.to_ascii_uppercase();
    if matches!(
        upper.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" | "CLOCK$"
    ) {
        return true;
    }
    let mut chars = stem.chars();
    let head: String = chars
        .by_ref()
        .take(3)
        .collect::<String>()
        .to_ascii_uppercase();
    let rest: String = chars.collect();
    (head == "COM" || head == "LPT")
        && matches!(
            rest.as_str(),
            "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
        )
}

/// 相対 path を検証して component に分ける（`/` と `\` のどちらも区切りとして扱う）。
///
/// 拒否するもの: 空、root 自身（`.`）、絶対 path、`..`、`:`（ドライブ・代替データストリーム）、
/// `\\?\` / UNC、予約名、末尾のドット・空白、制御文字、Windows で使えない文字（`<>"|?*`）
pub fn validate_relative(rel: &str) -> Result<Vec<String>> {
    if rel.is_empty() {
        return Err(deny("path is empty"));
    }
    if rel.chars().any(char::is_control) {
        return Err(deny(format!("path contains a control character: {rel:?}")));
    }
    if rel.starts_with(['/', '\\']) || Path::new(rel).is_absolute() {
        return Err(deny(format!(
            "absolute paths are not allowed: {rel:?} (use a path relative to the workspace root)"
        )));
    }
    if rel.contains(':') {
        return Err(deny(format!(
            "':' is not allowed in paths (drive letters, alternate data streams): {rel:?}"
        )));
    }
    if let Some(c) = rel
        .chars()
        .find(|c| matches!(c, '<' | '>' | '"' | '|' | '?' | '*'))
    {
        return Err(deny(format!("{c:?} is not allowed in paths: {rel:?}")));
    }
    let mut parts = vec![];
    for part in rel.split(['/', '\\']) {
        match part {
            "" | "." => continue,
            ".." => return Err(deny(format!("'..' is not allowed in paths: {rel:?}"))),
            _ => {}
        }
        if is_reserved_name(part) {
            return Err(deny(format!("reserved device name in path: {rel:?}")));
        }
        if part.ends_with(['.', ' ']) {
            return Err(deny(format!(
                "path components must not end with '.' or ' ': {rel:?}"
            )));
        }
        parts.push(part.to_string());
    }
    if parts.is_empty() {
        return Err(deny(format!(
            "path refers to the workspace root itself: {rel:?}"
        )));
    }
    Ok(parts)
}

/// open 済みの handle が指す実 path
#[cfg(windows)]
fn real_path(file: &File) -> std::io::Result<PathBuf> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_NAME_NORMALIZED, GetFinalPathNameByHandleW, VOLUME_NAME_DOS,
    };
    let handle = file.as_raw_handle();
    let mut buf = vec![0u16; 512];
    loop {
        let len = u32::try_from(buf.len()).unwrap_or(u32::MAX);
        // SAFETY: handle は file が持つ有効な handle、buf は len 要素の書き込み可能な領域
        let n = unsafe {
            GetFinalPathNameByHandleW(
                handle,
                buf.as_mut_ptr(),
                len,
                FILE_NAME_NORMALIZED | VOLUME_NAME_DOS,
            )
        } as usize;
        if n == 0 {
            return Err(std::io::Error::last_os_error());
        }
        if n < buf.len() {
            buf.truncate(n);
            return Ok(PathBuf::from(OsString::from_wide(&buf)));
        }
        // 足りなければ n（終端の NUL を含む必要な長さ）まで広げてやり直す
        buf.resize(n + 1, 0);
    }
}

#[cfg(target_os = "linux")]
fn real_path(file: &File) -> std::io::Result<PathBuf> {
    use std::os::fd::AsRawFd;
    std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))
}

#[cfg(target_os = "macos")]
fn real_path(file: &File) -> std::io::Result<PathBuf> {
    use std::ffi::CStr;
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    let mut buf = vec![0u8; libc::PATH_MAX as usize + 1];
    // SAFETY: F_GETPATH は PATH_MAX byte 以上の領域に NUL 終端の path を書く
    let r = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETPATH, buf.as_mut_ptr()) };
    if r == -1 {
        return Err(std::io::Error::last_os_error());
    }
    let c = CStr::from_bytes_until_nul(&buf).map_err(std::io::Error::other)?;
    Ok(PathBuf::from(std::ffi::OsStr::from_bytes(c.to_bytes())))
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn real_path(_file: &File) -> std::io::Result<PathBuf> {
    // 検証できない OS では開かない（fail closed）
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "cannot verify the real path of an open file on this OS",
    ))
}

/// ハードリンクの数
#[cfg(windows)]
fn link_count(file: &File) -> std::io::Result<u64> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    // SAFETY: 全 field が整数の C 構造体なので 0 埋めで有効
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: handle は file が持つ有効な handle、info は書き込み可能
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(u64::from(info.nNumberOfLinks))
}

#[cfg(unix)]
fn link_count(file: &File) -> std::io::Result<u64> {
    use std::os::unix::fs::MetadataExt;
    Ok(file.metadata()?.nlink())
}

#[cfg(not(any(windows, unix)))]
fn link_count(_file: &File) -> std::io::Result<u64> {
    Ok(1)
}

impl Workspace {
    /// roots を指定して作る（各 root は存在する絶対 path のディレクトリ）
    pub fn new(roots: Vec<PathBuf>) -> Result<Self> {
        Self::with_limits(roots, WorkspaceLimits::default())
    }

    pub fn with_limits(roots: Vec<PathBuf>, limits: WorkspaceLimits) -> Result<Self> {
        if roots.is_empty() {
            return Err(deny(roots_unset_message()));
        }
        let mut out = vec![];
        for root in roots {
            if !root.is_absolute() {
                return Err(deny(format!(
                    "workspace root must be an absolute path: {}",
                    root.display()
                )));
            }
            let canonical = std::fs::canonicalize(&root).map_err(|e| {
                deny(format!(
                    "workspace root is not accessible: {}: {e}",
                    root.display()
                ))
            })?;
            if !canonical.is_dir() {
                return Err(deny(format!(
                    "workspace root is not a directory: {}",
                    root.display()
                )));
            }
            out.push(Root {
                display: root,
                canonical,
            });
        }
        Ok(Workspace {
            roots: out,
            limits,
            changes: Mutex::new(vec![]),
        })
    }

    /// `SUBAGENT_ROOTS`（必須）と上限の env を読む
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let raw = get(ROOTS_ENV).filter(|v| !v.trim().is_empty());
        let Some(raw) = raw else {
            return Err(deny(roots_unset_message()));
        };
        let roots: Vec<PathBuf> = std::env::split_paths(raw.trim())
            .filter(|p| !p.as_os_str().is_empty())
            .collect();
        Self::with_limits(roots, WorkspaceLimits::from_lookup(get))
    }

    pub fn from_env() -> Result<Self> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// 最初の root（相対 path の基準、コマンドの cwd）。設定どおりの表記
    pub fn root(&self) -> &Path {
        &self.roots[0].display
    }

    /// 最初の root を canonicalize したもの
    pub fn canonical_root(&self) -> &Path {
        &self.roots[0].canonical
    }

    pub fn limits(&self) -> &WorkspaceLimits {
        &self.limits
    }

    fn lock(&self) -> MutexGuard<'_, Vec<FileChange>> {
        self.changes.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 作った / 変えたファイル（最初に書いた順）
    pub fn changes(&self) -> Vec<FileChange> {
        self.lock().clone()
    }

    fn ensure_inside(&self, real: &Path, rel: &str) -> Result<()> {
        if self.roots.iter().any(|r| real.starts_with(&r.canonical)) {
            Ok(())
        } else {
            Err(deny(format!(
                "{rel:?} resolves outside the workspace roots"
            )))
        }
    }

    /// open 済みの handle の実 path を確かめる（root の外なら拒否）
    pub fn verify_open_file(&self, file: &File) -> Result<PathBuf> {
        let real = real_path(file).map_err(|e| {
            deny(format!(
                "cannot verify the real path of an opened file: {e}"
            ))
        })?;
        self.ensure_inside(&real, &real.to_string_lossy())?;
        Ok(real)
    }

    /// 表示用の path（設定どおりの root + 相対 path）
    pub fn display_path(&self, rel: &str) -> Result<PathBuf> {
        let parts = validate_relative(rel)?;
        Ok(parts
            .iter()
            .fold(self.root().to_path_buf(), |p, c| p.join(c)))
    }

    /// 相対 path を検証して、canonical な root 基準の path にする。
    /// 最寄りの既存祖先を canonicalize して root の中にあることを確かめる
    pub fn resolve(&self, rel: &str) -> Result<PathBuf> {
        let parts = validate_relative(rel)?;
        self.resolve_parts(&parts, rel)
    }

    fn resolve_parts(&self, parts: &[String], rel: &str) -> Result<PathBuf> {
        let base = self.canonical_root();
        for i in (0..=parts.len()).rev() {
            let candidate = parts[..i].iter().fold(base.to_path_buf(), |p, c| p.join(c));
            if i > 0 && std::fs::symlink_metadata(&candidate).is_err() {
                continue;
            }
            // 壊れた link なども含め、解決できなければ拒否する
            let real = std::fs::canonicalize(&candidate)
                .map_err(|e| deny(format!("cannot resolve {rel:?}: {e}")))?;
            self.ensure_inside(&real, rel)?;
            return Ok(parts[i..].iter().fold(real, |p, c| p.join(c)));
        }
        unreachable!("i == 0 always returns")
    }

    /// コマンドの cwd（None は最初の root）。存在する root 内のディレクトリでなければ拒否
    pub fn resolve_dir(&self, rel: Option<&str>) -> Result<PathBuf> {
        let Some(rel) = rel else {
            return Ok(self.canonical_root().to_path_buf());
        };
        let p = self.resolve(rel)?;
        if !p.is_dir() {
            return Err(deny(format!("{rel:?} is not a directory in the workspace")));
        }
        Ok(p)
    }

    /// resolve 済みの path を読み出し用に開き、実 path・種類・サイズを確かめる
    pub fn open_resolved_for_read(&self, path: &Path) -> Result<File> {
        let file = File::open(path).map_err(|e| Error::io_at(path, e))?;
        self.verify_open_file(&file)?;
        let meta = file.metadata().map_err(|e| Error::io_at(path, e))?;
        if !meta.is_file() {
            return Err(deny(format!("not a regular file: {}", path.display())));
        }
        if meta.len() > self.limits.max_file_bytes {
            return Err(deny(format!(
                "file is larger than the limit ({} > {} bytes)",
                meta.len(),
                self.limits.max_file_bytes
            )));
        }
        Ok(file)
    }

    fn read_locked(&self, rel: &str) -> Result<String> {
        let parts = validate_relative(rel)?;
        let path = self.resolve_parts(&parts, rel)?;
        let shown = parts.join("/");
        // エラーには canonical な path ではなく、渡された相対 path を載せる
        let file = match self.open_resolved_for_read(&path) {
            Err(Error::IoPath { source, .. }) => return Err(Error::io_at(&shown, source)),
            other => other?,
        };
        let mut bytes = vec![];
        file.take(self.limits.max_file_bytes + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| Error::io_at(&shown, e))?;
        if bytes.len() as u64 > self.limits.max_file_bytes {
            return Err(deny(format!(
                "{shown} grew past the size limit while reading"
            )));
        }
        String::from_utf8(bytes)
            .map_err(|_| Error::InvalidParameter(format!("{shown} is not valid UTF-8 text")))
    }

    fn write_locked(
        &self,
        changes: &mut Vec<FileChange>,
        rel: &str,
        content: &str,
    ) -> Result<ChangeKind> {
        if content.len() as u64 > self.limits.max_file_bytes {
            return Err(deny(format!(
                "content is larger than the limit ({} > {} bytes)",
                content.len(),
                self.limits.max_file_bytes
            )));
        }
        let parts = validate_relative(rel)?;
        let shown = parts.join("/");
        let tracked = changes.iter().any(|c| c.path == shown);
        if !tracked && changes.len() >= self.limits.max_files {
            return Err(deny(format!(
                "too many files written in this workspace (limit {})",
                self.limits.max_files
            )));
        }
        let path = self.resolve_parts(&parts, rel)?;
        let existed = std::fs::symlink_metadata(&path).is_ok();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| Error::io_at(&shown, e))?;
            // 作っている間に差し替えられていないか
            let real = std::fs::canonicalize(dir).map_err(|e| Error::io_at(&shown, e))?;
            self.ensure_inside(&real, rel)?;
        }
        // truncate は検証の後（root の外の既存ファイルを空にしない）
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| Error::io_at(&shown, e))?;
        self.verify_open_file(&file)?;
        let meta = file.metadata().map_err(|e| Error::io_at(&shown, e))?;
        if !meta.is_file() {
            return Err(deny(format!("not a regular file: {shown}")));
        }
        if link_count(&file).map_err(|e| Error::io_at(&shown, e))? > 1 {
            return Err(deny(format!(
                "{shown} has more than one hard link; refusing to write"
            )));
        }
        file.set_len(0).map_err(|e| Error::io_at(&shown, e))?;
        file.write_all(content.as_bytes())
            .map_err(|e| Error::io_at(&shown, e))?;
        let kind = if existed {
            ChangeKind::Modified
        } else {
            ChangeKind::Created
        };
        if !tracked {
            changes.push(FileChange { path: shown, kind });
        }
        Ok(kind)
    }

    /// テキストとして読む（サイズ上限あり）
    pub fn read_text(&self, rel: &str) -> Result<String> {
        let _guard = self.lock();
        self.read_locked(rel)
    }

    /// ファイルを作る（あれば上書き）。親ディレクトリも作る
    pub fn create(&self, rel: &str, content: &str) -> Result<ChangeKind> {
        let mut changes = self.lock();
        self.write_locked(&mut changes, rel, content)
    }

    /// `old` がちょうど 1 回現れるときだけ `new` に置き換える（どちらもリテラル）
    pub fn str_replace(&self, rel: &str, old: &str, new: &str) -> Result<()> {
        if old.is_empty() {
            return Err(Error::InvalidParameter(
                "`old_str` must not be empty".into(),
            ));
        }
        let mut changes = self.lock();
        let content = self.read_locked(rel)?;
        match content.matches(old).count() {
            0 => Err(Error::InvalidParameter(format!(
                "No match found for `old_str` in {rel}"
            ))),
            1 => {
                let updated = content.replacen(old, new, 1);
                self.write_locked(&mut changes, rel, &updated).map(|_| ())
            }
            n => Err(Error::InvalidParameter(format!(
                "Found {n} matches for `old_str` in {rel}; it must match exactly once \
                 (add surrounding context)"
            ))),
        }
    }

    /// 重ならない一致をすべて置き換え、件数を返す（0 件なら書かない）。MCP の `str_replace` 用
    pub fn replace_all(&self, rel: &str, search: &str, replace: &str) -> Result<usize> {
        let mut changes = self.lock();
        let content = self.read_locked(rel)?;
        let n = if search.is_empty() {
            0
        } else {
            content.matches(search).count()
        };
        if n > 0 {
            let updated = content.replace(search, replace);
            self.write_locked(&mut changes, rel, &updated)?;
        }
        Ok(n)
    }

    /// `after_line` 行目の後ろ（0 なら先頭）に `text` を行として挿入する
    pub fn insert(&self, rel: &str, after_line: usize, text: &str) -> Result<()> {
        let mut changes = self.lock();
        let content = self.read_locked(rel)?;
        let lines: Vec<&str> = content.split_inclusive('\n').collect();
        if after_line > lines.len() {
            return Err(Error::InvalidParameter(format!(
                "insert_line {after_line} is past the end of {rel} ({} lines)",
                lines.len()
            )));
        }
        let mut out = lines[..after_line].concat();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(text);
        if !text.ends_with('\n') && (after_line < lines.len() || content.ends_with('\n')) {
            out.push('\n');
        }
        out.push_str(&lines[after_line..].concat());
        self.write_locked(&mut changes, rel, &out).map(|_| ())
    }

    /// 行番号付きで表示する。`range` は 1 始まりの (開始, 終了)。終了 None は末尾まで
    pub fn view(&self, rel: &str, range: Option<(usize, Option<usize>)>) -> Result<String> {
        let content = self.read_text(rel)?;
        let lines: Vec<&str> = content.lines().collect();
        let (start, end) = match range {
            None => (1, lines.len()),
            Some((s, e)) => {
                let e = e.unwrap_or(lines.len());
                if s == 0 || s > lines.len() || e < s {
                    return Err(Error::InvalidParameter(format!(
                        "invalid view_range [{s}, {e}] for {rel} ({} lines)",
                        lines.len()
                    )));
                }
                (s, e.min(lines.len()))
            }
        };
        let mut out = String::new();
        for (i, line) in lines.iter().enumerate().take(end).skip(start - 1) {
            out.push_str(&format!("{:>6}\t{line}\n", i + 1));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_names() {
        for n in [
            "CON", "con.txt", "Nul", "COM1", "lpt9.x.y", "CONOUT$", "aux ",
        ] {
            assert!(is_reserved_name(n), "{n}");
        }
        for n in ["CONSOLE", "COM10", "LPT0", "nulx", "a.con"] {
            assert!(!is_reserved_name(n), "{n}");
        }
    }

    #[test]
    fn limits_from_env() {
        let l = WorkspaceLimits::from_lookup(|k| match k {
            MAX_FILE_BYTES_ENV => Some("10".into()),
            MAX_FILES_ENV => Some("0".into()),
            _ => None,
        });
        assert_eq!(l.max_file_bytes, 10);
        assert_eq!(l.max_files, DEFAULT_MAX_FILES);
    }

    #[test]
    fn io_errors_use_the_relative_path() {
        let dir = tempfile::tempdir().unwrap();
        let w = Workspace::new(vec![dir.path().to_path_buf()]).unwrap();
        let e = w.read_text("definitely/missing.txt").unwrap_err();
        assert!(e.to_string().contains("definitely/missing.txt"), "{e}");
    }

    #[test]
    fn hard_links_are_not_written() {
        let root = tempfile::tempdir().unwrap();
        let other = root.path().join("other.txt");
        std::fs::write(&other, "keep").unwrap();
        if std::fs::hard_link(&other, root.path().join("h.txt")).is_err() {
            eprintln!("skip: hard links are not supported here");
            return;
        }
        let w = Workspace::new(vec![root.path().to_path_buf()]).unwrap();
        assert!(matches!(w.create("h.txt", "x"), Err(Error::Sandbox(_))));
        assert_eq!(std::fs::read_to_string(&other).unwrap(), "keep");
    }
}
