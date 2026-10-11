//! Integration tests for inline git blame.

use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn inline_blame_populates_for_tracked_file() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    init_git_repo(dir.path());

    let file = dir.path().join("file.txt");
    std::fs::write(&file, "hello\nworld\n")?;
    git(dir.path(), &["add", "file.txt"]);
    git(dir.path(), &["commit", "-m", "initial commit"]);

    let mut config = Config::default();
    config.editor.inline_blame = true;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_file(&file, None)
        .build()?;

    test_key_sequences(
        &mut app,
        vec![(
            None,
            Some(&|app| {
                let (view, doc) = helix_view::current_ref!(app.editor);
                let blame = doc
                    .blame()
                    .expect("blame should be loaded for a tracked file");
                assert_eq!(blame.len(), 2);
                assert_eq!(blame[0].author, "Test Author");
                assert_eq!(blame[0].summary, "initial commit");
                assert!(
                    doc.blame_annotations(view.id).is_some(),
                    "cursor line should have a blame annotation"
                );
            }),
        )],
        false,
    )
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn inline_blame_disabled_by_default() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    init_git_repo(dir.path());

    let file = dir.path().join("file.txt");
    std::fs::write(&file, "hello\n")?;
    git(dir.path(), &["add", "file.txt"]);
    git(dir.path(), &["commit", "-m", "initial commit"]);

    let mut app = AppBuilder::new().with_file(&file, None).build()?;

    test_key_sequences(
        &mut app,
        vec![(
            None,
            Some(&|app| {
                let (_, doc) = helix_view::current_ref!(app.editor);
                assert!(doc.blame().is_none(), "blame is opt-in");
            }),
        )],
        false,
    )
    .await?;

    Ok(())
}
