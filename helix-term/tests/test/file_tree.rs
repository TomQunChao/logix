use helix_term::{
    application::Application,
    ui::{EditorView, FileTree, GitStatus},
};
use helix_view::editor::{FileTreeOpenBehavior, FileTreeOpenTarget};

use super::*;

fn editor_view(app: &Application) -> &EditorView {
    app.compositor()
        .find_ref::<EditorView>()
        .expect("editor view should exist")
}

fn sidebar_open(app: &Application) -> bool {
    editor_view(app).sidebar.is_some()
}

#[tokio::test(flavor = "multi_thread")]
async fn file_tree_toggle_remembers_position() -> anyhow::Result<()> {
    // The default open target is the current file, so opt into the last state explicitly.
    let mut config = Config::default();
    config.editor.file_tree.open_target = FileTreeOpenTarget::LastState;
    let mut app = AppBuilder::new().with_config(config).build()?;

    test_key_sequences(
        &mut app,
        vec![
            // Open the file tree.
            (Some("<space>e"), Some(&|app| assert!(sidebar_open(app)))),
            // Move the selection down twice and close the tree.
            (Some("jj"), None),
            (
                Some("<space>e"),
                Some(&|app| {
                    assert!(!sidebar_open(app));
                    assert!(editor_view(app).file_tree_state.is_some());
                }),
            ),
            // Reopen: the tree returns to the remembered position.
            (
                Some("<space>e"),
                Some(&|app| {
                    assert!(sidebar_open(app));
                    let tree = editor_view(app).sidebar.as_ref().unwrap();
                    assert_eq!(tree.selected, 2);
                }),
            ),
            // Close with q: position is still remembered.
            (Some("q"), Some(&|app| assert!(!sidebar_open(app)))),
            (
                Some("<space>e"),
                Some(&|app| {
                    let tree = editor_view(app).sidebar.as_ref().unwrap();
                    assert_eq!(tree.selected, 2);
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn file_tree_select_and_select_extend() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;

    test_key_sequences(
        &mut app,
        vec![
            (Some("<space>e"), Some(&|app| assert!(sidebar_open(app)))),
            // Mark the current entry with v.
            (
                Some("v"),
                Some(&|app| {
                    assert_eq!(editor_view(app).sidebar.as_ref().unwrap().marked.len(), 1);
                }),
            ),
            // Move down twice and extend the selection with V: three entries
            // (the anchor plus the two below it) are marked.
            (
                Some("jjV"),
                Some(&|app| {
                    assert_eq!(editor_view(app).sidebar.as_ref().unwrap().marked.len(), 3);
                }),
            ),
            // Escape clears the marks first, the tree stays open.
            (
                Some("<esc>"),
                Some(&|app| {
                    assert!(sidebar_open(app));
                    assert!(editor_view(app).sidebar.as_ref().unwrap().marked.is_empty());
                }),
            ),
            // A second escape closes the tree.
            (Some("<esc>"), Some(&|app| assert!(!sidebar_open(app)))),
        ],
        false,
    )
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn file_tree_open_behavior_auto_closes_on_open() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;

    test_key_sequences(
        &mut app,
        vec![
            (Some("<space>e"), Some(&|app| assert!(sidebar_open(app)))),
            // Jump to the last entry (a file: directories sort first) and
            // open it; the tree closes automatically.
            (Some("G<ret>"), Some(&|app| assert!(!sidebar_open(app)))),
        ],
        false,
    )
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn file_tree_open_behavior_manual_stays_open() -> anyhow::Result<()> {
    let mut config = Config::default();
    config.editor.file_tree.open_behavior = FileTreeOpenBehavior::Manual;
    let mut app = AppBuilder::new().with_config(config).build()?;

    test_key_sequences(
        &mut app,
        vec![
            (Some("<space>e"), Some(&|app| assert!(sidebar_open(app)))),
            // Opening a file keeps the tree open with `manual` behavior.
            (Some("G<ret>"), Some(&|app| assert!(sidebar_open(app)))),
        ],
        false,
    )
    .await?;

    Ok(())
}

/// The workspace root helix would use, found by walking up for `.git`.
fn workspace_root() -> std::path::PathBuf {
    let mut dir = std::env::current_dir().expect("cwd");
    loop {
        if dir.join(".git").exists() {
            return dir;
        }
        if !dir.pop() {
            return std::env::current_dir().expect("cwd");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn file_tree_toggle_reveals_current_file_by_default() -> anyhow::Result<()> {
    let readme = workspace_root().join("README.md");
    if !readme.exists() {
        // Nothing to reveal in this environment.
        return Ok(());
    }

    let mut app = AppBuilder::new().with_file(readme.clone(), None).build()?;

    test_key_sequences(
        &mut app,
        vec![(
            Some("<space>e"),
            Some(&|app| {
                let tree = editor_view(app).sidebar.as_ref().unwrap();
                assert_eq!(tree.entries[tree.selected].path, readme);
            }),
        )],
        false,
    )
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn file_tree_toggle_hidden_hides_dotfiles() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;

    test_key_sequences(
        &mut app,
        vec![
            (
                Some("<space>e"),
                Some(&|app| {
                    let tree = editor_view(app).sidebar.as_ref().unwrap();
                    // The workspace root has dotfiles (e.g. `.git`).
                    assert!(tree.entries.iter().any(|entry| entry.name.starts_with('.')));
                }),
            ),
            (
                Some("."),
                Some(&|app| {
                    let tree = editor_view(app).sidebar.as_ref().unwrap();
                    assert!(tree
                        .entries
                        .iter()
                        .all(|entry| !entry.name.starts_with('.')));
                }),
            ),
            (
                Some("."),
                Some(&|app| {
                    let tree = editor_view(app).sidebar.as_ref().unwrap();
                    assert!(tree.entries.iter().any(|entry| entry.name.starts_with('.')));
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn file_tree_collapse_all() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;

    test_key_sequences(
        &mut app,
        vec![
            (Some("<space>e"), None),
            // Expand the first directory.
            (
                Some("l"),
                Some(&|app| {
                    let tree = editor_view(app).sidebar.as_ref().unwrap();
                    assert!(tree
                        .entries
                        .iter()
                        .any(|entry| entry.is_dir && entry.expanded));
                }),
            ),
            // `H` collapses everything.
            (
                Some("H"),
                Some(&|app| {
                    let tree = editor_view(app).sidebar.as_ref().unwrap();
                    assert!(tree.entries.iter().all(|entry| !entry.expanded));
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}

/// The git status the file tree attached to the entry with the given file name.
fn entry_status(tree: &FileTree, name: &str) -> Option<GitStatus> {
    tree.entries
        .iter()
        .find(|entry| entry.name == name)
        .and_then(|entry| entry.git_status)
}

/// The file tree must color real files by their git status. Build a temporary
/// repository, dirty its worktree and check that each entry carries the
/// matching status (the colors themselves are resolved from those statuses).
#[tokio::test(flavor = "multi_thread")]
async fn file_tree_entries_carry_git_status() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    // Canonicalize so the tree's `read_dir` paths match the paths git reports (on macOS the
    // temp directory is reached through a symlink, `/var` -> `/private/var`).
    let root = dir.path().canonicalize()?;
    init_git_repo(&root);

    // A committed baseline on which to build the statuses below.
    std::fs::write(root.join("committed.txt"), "one\n")?;
    std::fs::write(root.join("modified.txt"), "one\n")?;
    std::fs::write(root.join(".gitignore"), "ignored.txt\n")?;
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "initial"]);

    // Dirty the worktree: one modified, one untracked and one ignored file.
    std::fs::write(root.join("modified.txt"), "two\n")?;
    std::fs::write(root.join("untracked.txt"), "new\n")?;
    std::fs::write(root.join("ignored.txt"), "secret\n")?;

    let app = AppBuilder::new().build()?;
    let tree = FileTree::new(root.clone(), &app.editor);

    assert_eq!(entry_status(&tree, "committed.txt"), None);
    assert_eq!(
        entry_status(&tree, "modified.txt"),
        Some(GitStatus::Modified)
    );
    assert_eq!(
        entry_status(&tree, "untracked.txt"),
        Some(GitStatus::Untracked)
    );
    assert_eq!(entry_status(&tree, "ignored.txt"), Some(GitStatus::Ignored));

    Ok(())
}

/// Ignored directories are reported as a single entry by git; their children
/// must inherit the ignored status so the whole subtree is dimmed.
#[tokio::test(flavor = "multi_thread")]
async fn file_tree_ignored_directory_children_inherit_status() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    init_git_repo(&root);

    std::fs::write(root.join(".gitignore"), "target/\n")?;
    std::fs::create_dir(root.join("target"))?;
    std::fs::write(root.join("target/artifact.bin"), "x\n")?;
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "initial"]);

    let app = AppBuilder::new().build()?;
    let mut tree = FileTree::new(root.clone(), &app.editor);

    // `target` is reported as ignored, so expanding it must dim its children too.
    let target = tree
        .entries
        .iter()
        .position(|entry| entry.name == "target")
        .expect("target should be listed");
    assert_eq!(tree.entries[target].git_status, Some(GitStatus::Ignored));

    tree.toggle_expand(target);
    assert_eq!(
        entry_status(&tree, "artifact.bin"),
        Some(GitStatus::Ignored)
    );

    Ok(())
}

/// The git status map is cached per workspace root and refreshed in the background by a
/// filesystem watcher, so an external repository change shows up without reopening rescanning
/// from scratch.
#[tokio::test(flavor = "multi_thread")]
async fn file_tree_git_status_cache_refreshes_on_repository_change() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    init_git_repo(&root);

    std::fs::write(root.join("tracked.txt"), "one\n")?;
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "initial"]);

    let app = AppBuilder::new().build()?;

    // The first open scans the repository and caches the result.
    let tree = FileTree::new(root.clone(), &app.editor);
    assert_eq!(entry_status(&tree, "tracked.txt"), None);

    // Modify the repository outside the editor; the watcher must refresh the cache. The watcher
    // is registered on a background thread, so keep touching the file until an event lands (a
    // write that happens before the watch is active would otherwise be missed).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        let tree = FileTree::new(root.clone(), &app.editor);
        if entry_status(&tree, "tracked.txt") == Some(GitStatus::Modified) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "git status cache did not refresh after the repository changed"
        );
        std::fs::write(root.join("tracked.txt"), "two\n")?;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    Ok(())
}
