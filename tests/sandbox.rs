//! sandbox（Workspace / run_command）の結合テスト
//!
//! path の拒否表、symlink / junction での脱出、open 後の実 path 再検証（TOCTOU）、
//! サイズ・件数の上限、str_replace の「ちょうど 1 回」、コマンドの allowlist・timeout・出力上限・env。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use subagent::Error;
use subagent::sandbox::{
    ChangeKind, CommandPolicy, Sandbox, Workspace, WorkspaceLimits, child_env, run_command,
    split_command_line, validate_relative,
};

fn ws(dir: &Path) -> Workspace {
    Workspace::new(vec![dir.to_path_buf()]).unwrap()
}

fn is_sandbox_err(e: &Error) -> bool {
    matches!(e, Error::Sandbox(_))
}

// ---------------------------------------------------------------------------
// path の拒否表
// ---------------------------------------------------------------------------

#[test]
fn rejects_dangerous_paths() {
    let cases = [
        "",
        ".",
        "./",
        "/etc/passwd",
        "\\Windows\\win.ini",
        "C:\\Windows\\win.ini",
        "C:/Windows/win.ini",
        "c:relative.txt",
        "\\\\?\\C:\\x",
        "\\\\server\\share\\x",
        "//server/share/x",
        "..",
        "../x",
        "a/../../x",
        "a/..",
        "a\\..\\..\\x",
        "file.txt:stream",
        "file.txt::$DATA",
        "CON",
        "con",
        "nul.txt",
        "a/PRN.log",
        "AUX",
        "com1",
        "COM9.tar.gz",
        "LPT1",
        "lpt9.txt",
        "CONIN$",
        "a/NUL ",
        "trailing.",
        "trailing ",
        "nul\0byte",
    ];
    for c in cases {
        let r = validate_relative(c);
        assert!(r.is_err(), "should reject {c:?}: {r:?}");
        assert!(is_sandbox_err(&r.unwrap_err()), "{c:?}");
    }
}

#[test]
fn accepts_ordinary_relative_paths() {
    for (c, expected) in [
        ("a.txt", vec!["a.txt"]),
        ("src/lib.rs", vec!["src", "lib.rs"]),
        ("src\\lib.rs", vec!["src", "lib.rs"]),
        ("./src//lib.rs", vec!["src", "lib.rs"]),
        ("console.log", vec!["console.log"]),
        ("COM10", vec!["COM10"]),
        ("connect/nullable.rs", vec!["connect", "nullable.rs"]),
        (
            ".github/workflows/ci.yml",
            vec![".github", "workflows", "ci.yml"],
        ),
    ] {
        assert_eq!(validate_relative(c).unwrap(), expected, "{c:?}");
    }
}

#[test]
fn resolve_stays_inside_root() {
    let dir = tempfile::tempdir().unwrap();
    let w = ws(dir.path());
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let p = w.resolve("new/dir/file.txt").unwrap();
    assert!(p.starts_with(&root), "{p:?} vs {root:?}");
    assert!(w.resolve("../escape.txt").is_err());
}

#[test]
fn roots_must_be_absolute_existing_dirs_and_set() {
    let dir = tempfile::tempdir().unwrap();
    // 未設定・空
    let e = Workspace::from_lookup(|_| None).unwrap_err();
    assert!(e.to_string().contains("SUBAGENT_ROOTS"), "{e}");
    let e =
        Workspace::from_lookup(|k| (k == "SUBAGENT_ROOTS").then(|| "  ".to_string())).unwrap_err();
    assert!(e.to_string().contains("SUBAGENT_ROOTS"), "{e}");
    // 相対 path は cwd に依存するので拒否
    assert!(
        Workspace::from_lookup(|k| (k == "SUBAGENT_ROOTS").then(|| "rel/dir".to_string())).is_err()
    );
    // 存在しない
    let missing = dir.path().join("missing");
    let m = missing.to_string_lossy().into_owned();
    assert!(Workspace::from_lookup(|k| (k == "SUBAGENT_ROOTS").then(|| m.clone())).is_err());
    // 正しい値
    let d = dir.path().to_string_lossy().into_owned();
    let w = Workspace::from_lookup(|k| (k == "SUBAGENT_ROOTS").then(|| d.clone())).unwrap();
    assert_eq!(w.root(), dir.path());
}

// ---------------------------------------------------------------------------
// symlink / junction での脱出
// ---------------------------------------------------------------------------

/// dir への link を作る。OS が許さなければ false（テストは skip）
fn make_dir_link(target: &Path, link: &Path) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).is_ok()
    }
    #[cfg(windows)]
    {
        if std::os::windows::fs::symlink_dir(target, link).is_ok() {
            return true;
        }
        // symlink は開発者モード / 権限が要る。junction は一般ユーザーでも作れる
        std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
            && link.exists()
    }
}

fn make_file_link(target: &Path, link: &Path) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).is_ok()
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(target, link).is_ok()
    }
}

#[test]
fn dir_link_escape_is_rejected() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), "secret").unwrap();
    if !make_dir_link(outside.path(), &root.path().join("out")) {
        eprintln!("skip: OS refused to create a directory link");
        return;
    }
    let w = ws(root.path());
    for e in [
        w.read_text("out/secret.txt").unwrap_err(),
        w.create("out/new.txt", "x").unwrap_err(),
        w.resolve("out/secret.txt").unwrap_err(),
        w.resolve("out/not-yet/file.txt").unwrap_err(),
    ] {
        assert!(is_sandbox_err(&e), "{e}");
        assert_no_leak(&e, outside.path());
    }
    assert!(!outside.path().join("new.txt").exists());
}

#[test]
fn file_link_escape_is_rejected() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("secret.txt");
    std::fs::write(&secret, "secret").unwrap();
    if !make_file_link(&secret, &root.path().join("link.txt")) {
        eprintln!("skip: OS refused to create a file symlink");
        return;
    }
    let w = ws(root.path());
    assert!(is_sandbox_err(&w.read_text("link.txt").unwrap_err()));
    assert!(is_sandbox_err(&w.create("link.txt", "pwned").unwrap_err()));
    assert_eq!(std::fs::read_to_string(&secret).unwrap(), "secret");
}

// ---------------------------------------------------------------------------
// TOCTOU: open 後の実 path 再検証
// ---------------------------------------------------------------------------

#[test]
fn verify_open_file_rejects_handle_outside_root() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let w = ws(root.path());
    let inside = root.path().join("in.txt");
    std::fs::write(&inside, "a").unwrap();
    let real = w
        .verify_open_file(&std::fs::File::open(&inside).unwrap())
        .unwrap();
    assert!(real.ends_with("in.txt"), "{real:?}");

    let out = outside.path().join("out.txt");
    std::fs::write(&out, "b").unwrap();
    let e = w
        .verify_open_file(&std::fs::File::open(&out).unwrap())
        .unwrap_err();
    assert!(is_sandbox_err(&e), "{e}");
    assert_no_leak(&e, outside.path());
}

/// resolve した後に親 dir を外向きの link に差し替えても、open 後の再検証で止まる
#[test]
fn swap_after_resolve_is_caught_on_open() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("sub")).unwrap();
    std::fs::write(root.path().join("sub/f.txt"), "inside").unwrap();
    std::fs::write(outside.path().join("f.txt"), "outside").unwrap();
    let w = ws(root.path());
    let resolved = w.resolve("sub/f.txt").unwrap();

    // 差し替え（攻撃者の操作）
    std::fs::rename(root.path().join("sub"), root.path().join("sub-old")).unwrap();
    if !make_dir_link(outside.path(), &root.path().join("sub")) {
        eprintln!("skip: OS refused to create a directory link");
        return;
    }
    let e = w.open_resolved_for_read(&resolved).unwrap_err();
    assert!(is_sandbox_err(&e), "{e}");
    assert_no_leak(&e, outside.path());
}

/// エラーに root の外の実 path（の一部）を載せない
fn assert_no_leak(e: &Error, outside: &Path) {
    let name = outside.file_name().unwrap().to_string_lossy().into_owned();
    assert!(!e.to_string().contains(&name), "leaks {name}: {e}");
}

// ---------------------------------------------------------------------------
// サイズ・件数の上限と追跡
// ---------------------------------------------------------------------------

#[test]
fn size_and_count_limits() {
    let dir = tempfile::tempdir().unwrap();
    let w = Workspace::with_limits(
        vec![dir.path().to_path_buf()],
        WorkspaceLimits {
            max_file_bytes: 10,
            max_files: 2,
        },
    )
    .unwrap();
    assert!(is_sandbox_err(
        &w.create("big.txt", "01234567890").unwrap_err()
    ));
    assert!(!dir.path().join("big.txt").exists());
    w.create("a.txt", "0123456789").unwrap();
    w.create("b.txt", "x").unwrap();
    // 3 件目は拒否。既に追跡しているファイルの更新は通る
    assert!(is_sandbox_err(&w.create("c.txt", "x").unwrap_err()));
    assert!(!dir.path().join("c.txt").exists());
    w.create("a.txt", "y").unwrap();
    // 置換の結果が上限を超えるなら書かない
    assert!(is_sandbox_err(
        &w.str_replace("b.txt", "x", "0123456789AB").unwrap_err()
    ));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("b.txt")).unwrap(),
        "x"
    );
    // 大きすぎるファイルは読まない
    std::fs::write(dir.path().join("huge.txt"), "0123456789ABCDEF").unwrap();
    assert!(is_sandbox_err(&w.read_text("huge.txt").unwrap_err()));

    let changes = w.changes();
    assert_eq!(changes.len(), 2, "{changes:?}");
    assert_eq!(changes[0].path, "a.txt");
    assert_eq!(changes[0].kind, ChangeKind::Created);
    assert_eq!(changes[1].path, "b.txt");
}

#[test]
fn tracks_created_and_modified() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("old.txt"), "a\nb\n").unwrap();
    let w = ws(dir.path());
    w.create("new/x.txt", "x").unwrap();
    w.insert("old.txt", 1, "inserted").unwrap();
    let c = w.changes();
    assert_eq!(c[0].path, "new/x.txt");
    assert_eq!(c[0].kind, ChangeKind::Created);
    assert_eq!(c[1].path, "old.txt");
    assert_eq!(c[1].kind, ChangeKind::Modified);
}

// ---------------------------------------------------------------------------
// text editor 操作
// ---------------------------------------------------------------------------

#[test]
fn str_replace_requires_exactly_one_match() {
    let dir = tempfile::tempdir().unwrap();
    let w = ws(dir.path());
    w.create("f.txt", "foo foo bar").unwrap();
    let e = w.str_replace("f.txt", "foo", "X").unwrap_err();
    assert!(e.to_string().contains('2'), "{e}");
    let e = w.str_replace("f.txt", "nomatch", "X").unwrap_err();
    assert!(e.to_string().to_lowercase().contains("no match"), "{e}");
    assert!(w.str_replace("f.txt", "", "X").is_err());
    assert_eq!(w.read_text("f.txt").unwrap(), "foo foo bar");
    w.str_replace("f.txt", "bar", "$1").unwrap();
    assert_eq!(w.read_text("f.txt").unwrap(), "foo foo $1");
}

#[test]
fn insert_and_view() {
    let dir = tempfile::tempdir().unwrap();
    let w = ws(dir.path());
    w.create("f.txt", "a\nb\n").unwrap();
    w.insert("f.txt", 0, "top").unwrap();
    w.insert("f.txt", 3, "end").unwrap();
    assert_eq!(w.read_text("f.txt").unwrap(), "top\na\nb\nend\n");
    assert!(w.insert("f.txt", 99, "x").is_err());
    let v = w.view("f.txt", Some((2, Some(3)))).unwrap();
    assert_eq!(v, "     2\ta\n     3\tb\n");
    let v = w.view("f.txt", None).unwrap();
    assert!(v.starts_with("     1\ttop\n"), "{v}");
}

// ---------------------------------------------------------------------------
// run_command
// ---------------------------------------------------------------------------

#[test]
fn command_line_splitting() {
    assert_eq!(
        split_command_line("cargo clippy -- -D warnings").unwrap(),
        (
            "cargo".into(),
            vec!["clippy".into(), "--".into(), "-D".into(), "warnings".into()]
        )
    );
    assert_eq!(
        split_command_line(r#"echo "a b" 'c d'"#).unwrap(),
        ("echo".into(), vec!["a b".into(), "c d".into()])
    );
    // shell の構文は解釈しない（拒否する）
    for bad in [
        "",
        "  ",
        "a | b",
        "a && b",
        "a; b",
        "a > f",
        "echo $(id)",
        "echo `id`",
        "a\nb",
        "echo \"unterminated",
    ] {
        assert!(split_command_line(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn allowlist_parsing() {
    let p = CommandPolicy::from_lookup(|k| {
        (k == "SUBAGENT_CODEGEN_ALLOWED_COMMANDS")
            .then(|| " cargo, npm ,/bin/sh,..\\x, ,git".to_string())
    });
    assert!(p.is_enabled());
    assert!(p.check_program("cargo").is_ok());
    assert!(p.check_program("git").is_ok());
    assert!(p.check_program("sh").is_err());
    assert!(p.check_program("/bin/sh").is_err());
    assert!(p.check_program("rm").is_err());
    assert!(p.check_program("./cargo").is_err());
    assert_eq!(p.timeout(), Duration::from_secs(120));

    let p = CommandPolicy::from_lookup(|k| match k {
        "SUBAGENT_CODEGEN_ALLOWED_COMMANDS" => Some("cargo".into()),
        "SUBAGENT_COMMAND_TIMEOUT_SECS" => Some("5".into()),
        _ => None,
    });
    assert_eq!(p.timeout(), Duration::from_secs(5));
}

#[tokio::test]
async fn command_disabled_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let p = CommandPolicy::from_lookup(|_| None);
    assert!(!p.is_enabled());
    let e = run_command(&p, dir.path(), "cargo", &["--version".to_string()])
        .await
        .unwrap_err();
    assert!(
        e.to_string().contains("SUBAGENT_CODEGEN_ALLOWED_COMMANDS"),
        "{e}"
    );
}

#[tokio::test]
async fn non_allowlisted_program_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let p = CommandPolicy::allow(&["cargo"]);
    for prog in ["rustc", "sh", "cmd", "/bin/echo", "..\\cargo"] {
        let e = run_command(&p, dir.path(), prog, &[]).await.unwrap_err();
        assert!(is_sandbox_err(&e), "{prog}: {e}");
    }
}

#[cfg(windows)]
fn sleeper() -> (&'static str, Vec<String>) {
    ("ping", vec!["-n".into(), "30".into(), "127.0.0.1".into()])
}
#[cfg(not(windows))]
fn sleeper() -> (&'static str, Vec<String>) {
    ("sleep", vec!["30".into()])
}

#[tokio::test]
async fn timeout_kills_the_child() {
    let dir = tempfile::tempdir().unwrap();
    let (prog, args) = sleeper();
    let p = CommandPolicy::allow(&[prog]).with_timeout(Duration::from_millis(500));
    let start = Instant::now();
    let out = run_command(&p, dir.path(), prog, &args).await.unwrap();
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "{:?}",
        start.elapsed()
    );
    assert!(out.timed_out);
    assert!(!out.success);
}

#[cfg(windows)]
fn noisy() -> (&'static str, Vec<String>) {
    (
        "powershell",
        vec![
            "-NoProfile".into(),
            "-Command".into(),
            "'x' * 100000".into(),
        ],
    )
}
#[cfg(not(windows))]
fn noisy() -> (&'static str, Vec<String>) {
    ("seq", vec!["1".into(), "30000".into()])
}

#[tokio::test]
async fn output_is_truncated() {
    let dir = tempfile::tempdir().unwrap();
    let (prog, args) = noisy();
    let p = CommandPolicy::allow(&[prog]);
    let out = run_command(&p, dir.path(), prog, &args).await.unwrap();
    assert!(out.success, "{out:?}");
    assert!(out.truncated);
    assert!(out.stdout.chars().count() <= subagent::sandbox::OUTPUT_LIMIT_CHARS + 200);
    assert!(out.stdout.contains("truncated"));
}

#[test]
fn child_env_keeps_only_passthrough_vars() {
    let abs = if cfg!(windows) { "C:\\bin" } else { "/usr/bin" };
    let parent = [
        ("PATH", abs),
        ("HOME", "/home/u"),
        ("TEMP", "/tmp"),
        ("ANTHROPIC_API_KEY", "sk-secret"),
        ("SUBAGENT_ANTHROPIC_API_KEY", "sk-secret2"),
        ("NOTION_TOKEN", "ntn"),
        ("AWS_SECRET_ACCESS_KEY", "aws"),
        // Windows の tool chain が使う、secret でない変数
        ("APPDATA", "C:\\Users\\u\\AppData\\Roaming"),
        ("LOCALAPPDATA", "C:\\Users\\u\\AppData\\Local"),
        ("ProgramData", "C:\\ProgramData"),
        ("SystemDrive", "C:"),
        ("PATHEXT", ".COM;.EXE"),
        ("ComSpec", "C:\\Windows\\system32\\cmd.exe"),
    ];
    let env = child_env(|k| {
        parent
            .iter()
            .find(|(n, _)| *n == k)
            .map(|(_, v)| v.to_string())
    });
    let names: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
    for keep in [
        "PATH",
        "HOME",
        "TEMP",
        "APPDATA",
        "LOCALAPPDATA",
        "ProgramData",
        "SystemDrive",
        "PATHEXT",
        "ComSpec",
    ] {
        assert!(names.contains(&keep), "{keep} missing: {names:?}");
    }
    for secret in [
        "ANTHROPIC_API_KEY",
        "SUBAGENT_ANTHROPIC_API_KEY",
        "NOTION_TOKEN",
        "AWS_SECRET_ACCESS_KEY",
    ] {
        assert!(!names.contains(&secret), "{secret} leaked: {names:?}");
    }
}

#[test]
fn child_path_drops_relative_entries() {
    let sep = if cfg!(windows) { ";" } else { ":" };
    let abs = if cfg!(windows) { "C:\\bin" } else { "/usr/bin" };
    let path = format!("{abs}{sep}.{sep}{sep}rel/bin");
    let env = child_env(|k| (k == "PATH").then(|| path.clone()));
    let p = &env.iter().find(|(k, _)| k == "PATH").unwrap().1;
    let parts: Vec<PathBuf> = std::env::split_paths(p).collect();
    assert_eq!(parts, vec![PathBuf::from(abs)]);
}

// ---------------------------------------------------------------------------
// critic 指摘への対応（PR #77）
// ---------------------------------------------------------------------------

/// FIFO は開かずに拒否する（以前は open が永久に止まり、lock も握ったままになった）
#[cfg(unix)]
#[test]
fn fifo_is_refused_without_hanging() {
    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("pipe");
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .is_ok_and(|s| s.success());
    if !made {
        eprintln!("skip: mkfifo is not available");
        return;
    }
    let root = dir.path().to_path_buf();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let w = ws(&root);
        let read = w.read_text("pipe").map(|_| ());
        let write = w.create("pipe", "x").map(|_| ());
        let _ = tx.send((read, write));
    });
    let (read, write) = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("workspace hung on a FIFO");
    assert!(matches!(read, Err(Error::Sandbox(_))), "{read:?}");
    assert!(matches!(write, Err(Error::Sandbox(_))), "{write:?}");
}

/// 件数の上限はタスクごと（begin_task で数え直す）
#[test]
fn file_limit_is_per_task() {
    let dir = tempfile::tempdir().unwrap();
    let w = Workspace::new(vec![dir.path().to_path_buf()]).unwrap();
    let max = subagent::sandbox::workspace::DEFAULT_MAX_FILES;
    // 1 タスクで上限を超えると拒否
    w.begin_task();
    for i in 0..max {
        w.create(&format!("one/{i}.txt"), "x").unwrap();
    }
    assert!(is_sandbox_err(&w.create("one/over.txt", "x").unwrap_err()));
    // タスクを分ければ合計で上限を超えても書ける
    for i in 0..(max + 5) {
        w.begin_task();
        w.create(&format!("many/{i}.txt"), "x").unwrap();
    }
    // changes() は今のタスクの分だけ
    assert_eq!(w.changes().len(), 1);
}

#[test]
fn resolve_dir_accepts_root_aliases() {
    let dir = tempfile::tempdir().unwrap();
    let w = ws(dir.path());
    let root = std::fs::canonicalize(dir.path()).unwrap();
    for rel in [".", "", "./", "  "] {
        assert_eq!(w.resolve_dir(Some(rel)).unwrap(), root, "{rel:?}");
    }
    assert!(w.resolve_dir(Some("..")).is_err());
}

/// 読み出しもハードリンク（リンク数 2 以上）を拒否する（書き込みと同じ）
#[test]
fn hard_linked_files_are_not_read() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a.txt");
    std::fs::write(&a, "data").unwrap();
    if std::fs::hard_link(&a, root.path().join("b.txt")).is_err() {
        eprintln!("skip: hard links are not supported here");
        return;
    }
    let w = ws(root.path());
    assert!(is_sandbox_err(&w.read_text("b.txt").unwrap_err()));
    assert!(is_sandbox_err(&w.read_text("a.txt").unwrap_err()));
}

/// PATH の entry が workspace の中を指していれば子に渡さない（workspace に置いた偽の program を防ぐ）
#[test]
fn child_path_drops_entries_inside_the_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let outside = tempfile::tempdir().unwrap();
    let path = std::env::join_paths([bin.clone(), outside.path().to_path_buf()]).unwrap();
    let path = path.to_string_lossy().into_owned();
    let sb = Sandbox::new(
        ws(dir.path()),
        CommandPolicy::from_lookup(|k| match k {
            "PATH" => Some(path.clone()),
            "SUBAGENT_CODEGEN_ALLOWED_COMMANDS" => Some("cargo".into()),
            _ => None,
        }),
    );
    let env = sb.effective_env().unwrap();
    let p = &env.iter().find(|(k, _)| k == "PATH").unwrap().1;
    let parts: Vec<PathBuf> = std::env::split_paths(p).collect();
    assert_eq!(parts, vec![outside.path().to_path_buf()]);
}

/// Windows では `.exe` 以外の拡張子（`.bat` / `.cmd`）は allowlist にも program にも使えない
#[cfg(windows)]
#[test]
fn windows_allowlist_rejects_script_extensions() {
    let p = CommandPolicy::from_lookup(|k| {
        (k == "SUBAGENT_CODEGEN_ALLOWED_COMMANDS").then(|| "npm.cmd,build.bat,git.exe,cargo".into())
    });
    assert_eq!(p.allowed(), ["git".to_string(), "cargo".to_string()]);
    assert!(p.check_program("git").is_ok());
    assert!(p.check_program("GIT.EXE").is_ok());
    assert!(p.check_program("cargo.cmd").is_err());
    assert!(p.check_program("npm.cmd").is_err());
    assert!(p.check_program("build.bat").is_err());
}
