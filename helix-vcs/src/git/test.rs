use std::{fs::File, io::Write, path::Path, process::Command};

use tempfile::TempDir;

use crate::{git, FileChange};

fn exec_git_cmd(args: &str, git_dir: &Path) {
    let res = Command::new("git")
        .arg("-C")
        .arg(git_dir) // execute the git command in this directory
        .args(args.split_whitespace())
        .env_remove("GIT_DIR")
        .env_remove("GIT_ASKPASS")
        .env_remove("SSH_ASKPASS")
        .env("GIT_TERMINAL_PROMPT", "false")
        .env("GIT_AUTHOR_DATE", "2000-01-01 00:00:00 +0000")
        .env("GIT_AUTHOR_EMAIL", "author@example.com")
        .env("GIT_AUTHOR_NAME", "author")
        .env("GIT_COMMITTER_DATE", "2000-01-02 00:00:00 +0000")
        .env("GIT_COMMITTER_EMAIL", "committer@example.com")
        .env("GIT_COMMITTER_NAME", "committer")
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
        .env("GIT_CONFIG_VALUE_0", "false")
        .env("GIT_CONFIG_KEY_1", "init.defaultBranch")
        .env("GIT_CONFIG_VALUE_1", "main")
        .output()
        .unwrap_or_else(|_| panic!("`git {args}` failed"));
    if !res.status.success() {
        println!("{}", String::from_utf8_lossy(&res.stdout));
        eprintln!("{}", String::from_utf8_lossy(&res.stderr));
        panic!("`git {args}` failed (see output above)")
    }
}

fn create_commit(repo: &Path, add_modified: bool) {
    if add_modified {
        exec_git_cmd("add -A", repo);
    }
    exec_git_cmd("commit -m message", repo);
}

fn empty_git_repo() -> TempDir {
    let tmp = tempfile::tempdir().expect("create temp dir for git testing");
    exec_git_cmd("init", tmp.path());
    exec_git_cmd("config user.email test@helix.org", tmp.path());
    exec_git_cmd("config user.name helix-test", tmp.path());
    tmp
}

#[test]
fn missing_file() {
    let temp_git = empty_git_repo();
    let file = temp_git.path().join("file.txt");
    File::create(&file).unwrap().write_all(b"foo").unwrap();

    assert!(git::get_diff_base(&file, true).is_err());
}

#[test]
fn unmodified_file() {
    let temp_git = empty_git_repo();
    let file = temp_git.path().join("file.txt");
    let contents = b"foo".as_slice();
    File::create(&file).unwrap().write_all(contents).unwrap();
    create_commit(temp_git.path(), true);
    assert_eq!(
        git::get_diff_base(&file, true).unwrap(),
        Vec::from(contents)
    );
}

#[test]
fn modified_file() {
    let temp_git = empty_git_repo();
    let file = temp_git.path().join("file.txt");
    let contents = b"foo".as_slice();
    File::create(&file).unwrap().write_all(contents).unwrap();
    create_commit(temp_git.path(), true);
    File::create(&file).unwrap().write_all(b"bar").unwrap();

    assert_eq!(
        git::get_diff_base(&file, true).unwrap(),
        Vec::from(contents)
    );
}

/// Test that `get_file_head` does not return content for a directory.
/// This is important to correctly cover cases where a directory is removed and replaced by a file.
/// If the contents of the directory object were returned a diff between a path and the directory children would be produced.
#[test]
fn directory() {
    let temp_git = empty_git_repo();
    let dir = temp_git.path().join("file.txt");
    std::fs::create_dir(&dir).expect("");
    let file = dir.join("file.txt");
    let contents = b"foo".as_slice();
    File::create(file).unwrap().write_all(contents).unwrap();

    create_commit(temp_git.path(), true);

    std::fs::remove_dir_all(&dir).unwrap();
    File::create(&dir).unwrap().write_all(b"bar").unwrap();
    assert!(git::get_diff_base(&dir, true).is_err());
}

/// Test that `get_diff_base` resolves symlinks so that the same diff base is
/// used as the target file.
///
/// This is important to correctly cover cases where a symlink is removed and
/// replaced by a file. If the contents of the symlink object were returned
/// a diff between a literal file path and the actual file content would be
/// produced (bad ui).
#[cfg(any(unix, windows))]
#[test]
fn symlink() {
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    #[cfg(not(unix))]
    use std::os::windows::fs::symlink_file as symlink;

    let temp_git = empty_git_repo();
    let file = temp_git.path().join("file.txt");
    let contents = Vec::from(b"foo");
    File::create(&file).unwrap().write_all(&contents).unwrap();
    let file_link = temp_git.path().join("file_link.txt");

    symlink("file.txt", &file_link).unwrap();
    create_commit(temp_git.path(), true);

    assert_eq!(git::get_diff_base(&file_link, true).unwrap(), contents);
    assert_eq!(git::get_diff_base(&file, true).unwrap(), contents);
}

/// Test that `get_diff_base` returns content when the file is a symlink to
/// another file that is in a git repo, but the symlink itself is not.
#[cfg(any(unix, windows))]
#[test]
fn symlink_to_git_repo() {
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    #[cfg(not(unix))]
    use std::os::windows::fs::symlink_file as symlink;

    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let temp_git = empty_git_repo();

    let file = temp_git.path().join("file.txt");
    let contents = Vec::from(b"foo");
    File::create(&file).unwrap().write_all(&contents).unwrap();
    create_commit(temp_git.path(), true);

    let file_link = temp_dir.path().join("file_link.txt");
    symlink(&file, &file_link).unwrap();

    assert_eq!(git::get_diff_base(&file_link, true).unwrap(), contents);
    assert_eq!(git::get_diff_base(&file, true).unwrap(), contents);
}

fn collect_changes(repo: &Path, include_ignored: bool) -> Vec<FileChange> {
    let changes = std::cell::RefCell::new(Vec::new());
    git::for_each_changed_file(repo, true, include_ignored, |change| {
        changes
            .borrow_mut()
            .push(change.expect("status should not error"));
        true
    })
    .expect("git status should succeed");
    changes.into_inner()
}

#[test]
fn staged_addition_is_reported_as_added() {
    let temp_git = empty_git_repo();
    let tracked = temp_git.path().join("tracked.txt");
    File::create(&tracked).unwrap().write_all(b"foo").unwrap();
    create_commit(temp_git.path(), true);

    let new_file = temp_git.path().join("new.txt");
    File::create(&new_file).unwrap().write_all(b"bar").unwrap();
    exec_git_cmd("add new.txt", temp_git.path());

    let changes = collect_changes(temp_git.path(), false);
    assert!(
        changes
            .iter()
            .any(|c| matches!(c, FileChange::Added { path } if path == &new_file)),
        "expected {new_file:?} to be reported as Added, got {changes:?}"
    );
}

#[test]
fn ignored_file_is_reported_only_when_requested() {
    let temp_git = empty_git_repo();
    let tracked = temp_git.path().join("tracked.txt");
    File::create(&tracked).unwrap().write_all(b"foo").unwrap();
    let gitignore = temp_git.path().join(".gitignore");
    File::create(&gitignore)
        .unwrap()
        .write_all(b"*.log\n")
        .unwrap();
    create_commit(temp_git.path(), true);

    let ignored = temp_git.path().join("debug.log");
    File::create(&ignored).unwrap().write_all(b"junk").unwrap();

    let without = collect_changes(temp_git.path(), false);
    assert!(
        !without
            .iter()
            .any(|c| matches!(c, FileChange::Ignored { .. })),
        "ignored files must not be reported unless requested, got {without:?}"
    );

    let with = collect_changes(temp_git.path(), true);
    assert!(
        with.iter()
            .any(|c| matches!(c, FileChange::Ignored { path } if path == &ignored)),
        "expected {ignored:?} to be reported as Ignored, got {with:?}"
    );
}

#[test]
fn untracked_file_is_reported() {
    let temp_git = empty_git_repo();
    let tracked = temp_git.path().join("tracked.txt");
    File::create(&tracked).unwrap().write_all(b"foo").unwrap();
    create_commit(temp_git.path(), true);

    let untracked = temp_git.path().join("fresh.txt");
    File::create(&untracked).unwrap().write_all(b"new").unwrap();

    let changes = collect_changes(temp_git.path(), false);
    assert!(
        changes
            .iter()
            .any(|c| matches!(c, FileChange::Untracked { path } if path == &untracked)),
        "expected {untracked:?} to be reported as Untracked, got {changes:?}"
    );
}
