//! codegen が使う Workspace の呼び出しごとの書き込み（`*_tracked`）
//!
//! - 書いてよいかの確認（`may_write`）は Workspace の lock の中で、書く直前に呼ぶ（F5）
//! - `.git` の下には、どの書き込みの経路でも書かない

use subagent::Error;
use subagent::sandbox::{ChangeKind, FileChange, Workspace};

fn ws(dir: &std::path::Path) -> Workspace {
    Workspace::new(vec![dir.to_path_buf()]).unwrap()
}

fn deny() -> std::result::Result<(), String> {
    Err("deadline exceeded; nothing was written".into())
}

fn allow() -> std::result::Result<(), String> {
    Ok(())
}

#[test]
fn may_write_refusal_writes_nothing_not_even_directories() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
    let w = ws(dir.path());
    let mut log: Vec<FileChange> = vec![];
    let e = w
        .create_tracked(&mut log, &deny, "new/dir/x.txt", "x")
        .unwrap_err();
    assert!(e.to_string().contains("deadline"), "{e}");
    assert!(!dir.path().join("new").exists());
    assert!(
        w.str_replace_tracked(&mut log, &deny, "a.txt", "one", "1")
            .is_err()
    );
    assert!(
        w.insert_tracked(&mut log, &deny, "a.txt", 0, "zero")
            .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "one\n"
    );
    assert!(log.is_empty());
    assert!(w.changes().is_empty());
}

/// may_write は lock を取った後に呼ばれる（呼ばれている間、別 thread の操作は lock を待つ）
#[test]
fn may_write_runs_inside_the_workspace_lock() {
    let dir = tempfile::tempdir().unwrap();
    let w = std::sync::Arc::new(ws(dir.path()));
    let mut log = vec![];
    let w2 = w.clone();
    let other = std::sync::Mutex::new(None);
    let probe = || {
        let w3 = w2.clone();
        let t = std::thread::spawn(move || w3.create("other.txt", "o").unwrap());
        std::thread::sleep(std::time::Duration::from_millis(300));
        let finished = t.is_finished();
        *other.lock().unwrap() = Some(t);
        if finished {
            Err("may_write was called outside the lock".to_string())
        } else {
            Ok(())
        }
    };
    w.create_tracked(&mut log, &probe, "x.txt", "x").unwrap();
    // lock を離した後なら、待たせた操作も終わる
    other.lock().unwrap().take().unwrap().join().unwrap();
    assert!(dir.path().join("other.txt").exists());
    assert_eq!(
        log,
        vec![FileChange {
            path: "x.txt".into(),
            kind: ChangeKind::Created
        }]
    );
}

#[test]
fn tracked_writes_succeed_when_allowed() {
    let dir = tempfile::tempdir().unwrap();
    let w = ws(dir.path());
    let mut log = vec![];
    w.create_tracked(&mut log, &allow, "a.txt", "one\n")
        .unwrap();
    w.str_replace_tracked(&mut log, &allow, "a.txt", "one", "1")
        .unwrap();
    w.insert_tracked(&mut log, &allow, "a.txt", 1, "2").unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "1\n2\n"
    );
    assert_eq!(log.len(), 1);
    assert!(w.changes().is_empty());
}

#[test]
fn no_write_path_reaches_dot_git() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".git")).unwrap();
    std::fs::write(dir.path().join(".git/config"), "[core]\n").unwrap();
    let w = ws(dir.path());
    let mut log = vec![];
    let is_git_denial = |e: Error| matches!(&e, Error::Sandbox(m) if m.contains(".git"));
    for p in [".git/hooks/pre-commit", ".GIT/config", "x/.gIt/y", ".git"] {
        assert!(is_git_denial(w.create(p, "evil").unwrap_err()), "{p}");
        assert!(
            is_git_denial(w.create_tracked(&mut log, &allow, p, "evil").unwrap_err()),
            "{p}"
        );
    }
    assert!(is_git_denial(
        w.str_replace(".git/config", "[core]", "x").unwrap_err()
    ));
    assert!(is_git_denial(
        w.replace_all(".git/config", "[core]", "x").unwrap_err()
    ));
    assert!(is_git_denial(w.insert(".git/config", 0, "x").unwrap_err()));
    assert!(is_git_denial(
        w.str_replace_tracked(&mut log, &allow, ".Git/config", "[core]", "x")
            .unwrap_err()
    ));
    assert!(is_git_denial(
        w.insert_tracked(&mut log, &allow, ".git/config", 0, "x")
            .unwrap_err()
    ));
    // 無いファイルでも（大文字小文字を区別する FS で `.Git/config` が無い時も）、
    // 読み込みの not found より先に .git の拒否になる
    for p in [".GIT/missing", "x/.gIt/nope", ".git/nope"] {
        assert!(
            is_git_denial(w.str_replace(p, "a", "b").unwrap_err()),
            "{p}"
        );
        assert!(
            is_git_denial(w.replace_all(p, "a", "b").unwrap_err()),
            "{p}"
        );
        assert!(is_git_denial(w.insert(p, 0, "x").unwrap_err()), "{p}");
        assert!(
            is_git_denial(
                w.str_replace_tracked(&mut log, &allow, p, "a", "b")
                    .unwrap_err()
            ),
            "{p}"
        );
        assert!(
            is_git_denial(w.insert_tracked(&mut log, &allow, p, 0, "x").unwrap_err()),
            "{p}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(dir.path().join(".git/config")).unwrap(),
        "[core]\n"
    );
    assert!(!dir.path().join(".git/hooks").exists());
    assert!(!dir.path().join("x").exists());
    // 読むのは今までどおり
    assert_eq!(w.read_text(".git/config").unwrap(), "[core]\n");
}

/// root 自体が .git の中にあっても、判定は root からの相対 component だけで行う
#[test]
fn root_inside_a_dot_git_directory_is_not_blocked() {
    let outer = tempfile::tempdir().unwrap();
    let root = outer.path().join(".git").join("work");
    std::fs::create_dir_all(&root).unwrap();
    ws(&root).create("a.txt", "x").unwrap();
}
