//! Integration tests for project-wide search and replace.

use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn global_replace_rewrites_workspace_files() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    std::fs::create_dir(dir.path().join("sub"))?;
    std::fs::write(dir.path().join("a.txt"), "foo foo\n")?;
    std::fs::write(dir.path().join("b.txt"), "no match here\n")?;
    std::fs::write(dir.path().join("sub/c.txt"), "before foo after\n")?;

    let mut app = AppBuilder::new().build()?;

    let (changed, matches) = helix_term::commands::apply_global_replace_at(
        &mut app.editor,
        dir.path(),
        "foo",
        "bar",
        false,
    )?;

    assert_eq!(changed, 2, "two files should have been rewritten");
    assert_eq!(matches, 3, "three occurrences should have been replaced");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.txt"))?,
        "bar bar\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("b.txt"))?,
        "no match here\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("sub/c.txt"))?,
        "before bar after\n"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn global_replace_supports_capture_groups_and_smart_case() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("a.txt"), "Foo foo\n")?;

    let mut app = AppBuilder::new().build()?;

    // All-lowercase pattern with smart case is case-insensitive.
    let (changed, matches) = helix_term::commands::apply_global_replace_at(
        &mut app.editor,
        dir.path(),
        "foo",
        "bar",
        true,
    )?;
    assert_eq!((changed, matches), (1, 2));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.txt"))?,
        "bar bar\n"
    );

    // Capture group replacement.
    std::fs::write(dir.path().join("a.txt"), "abc def\n")?;
    let (_, matches) = helix_term::commands::apply_global_replace_at(
        &mut app.editor,
        dir.path(),
        r"(\w+)",
        r"[$1]",
        false,
    )?;
    assert_eq!(matches, 2);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.txt"))?,
        "[abc] [def]\n"
    );

    Ok(())
}
