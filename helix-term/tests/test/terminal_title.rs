use helix_view::doc;

use super::*;

/// Regression test for a startup panic at `helix-view/src/tree.rs` (`Tree::get`
/// unwrapping `None`).
///
/// Opening a file from the command line dispatches `DocumentDidOpen` *before* the
/// document's view is inserted into the tree, so the tree focus still points at the
/// root container. The terminal-title hook runs on that event and used to resolve
/// the focused view with `Tree::get`, which panicked. Building the application must
/// therefore succeed.
#[tokio::test(flavor = "multi_thread")]
async fn opening_a_file_at_startup_builds_the_title_without_panicking() -> anyhow::Result<()> {
    let file = temp_file_with_contents("fn main() {}\n")?;
    let mut app = AppBuilder::new()
        .with_file(file.path().to_path_buf(), None)
        .build()?;

    // The file was opened and is focused, so the title has something to resolve to.
    let doc = doc!(app.editor);
    assert_eq!(doc.path(), Some(file.path()));

    test_key_sequences(&mut app, vec![(None, None)], false).await?;

    Ok(())
}
