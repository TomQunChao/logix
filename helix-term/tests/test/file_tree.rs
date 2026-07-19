use helix_term::{application::Application, ui::EditorView};
use helix_view::editor::FileTreeOpenBehavior;

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
    let mut app = AppBuilder::new().build()?;

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
