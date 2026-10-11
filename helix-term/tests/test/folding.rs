//! Integration tests for code folding.

use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn fold_toggle_and_open_all() -> anyhow::Result<()> {
    let input = "#[|]#fn foo() {\n    a();\n    b();\n}\nc();\n";
    let mut app = AppBuilder::new().with_input_text(input).build()?;

    test_key_sequences(
        &mut app,
        vec![
            (
                Some("za"),
                Some(&|app| {
                    let (view, doc) = helix_view::current_ref!(app.editor);
                    let folds = doc.collapsed_folds(view.id).unwrap_or(&[]);
                    assert_eq!(folds.len(), 1, "za should collapse the enclosing block");
                }),
            ),
            (
                Some("zR"),
                Some(&|app| {
                    let (view, doc) = helix_view::current_ref!(app.editor);
                    assert!(
                        doc.collapsed_folds(view.id).is_none(),
                        "zR should open all folds"
                    );
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn fold_close_all_collapses_indented_blocks() -> anyhow::Result<()> {
    let input = "#[|]#fn foo() {\n    a();\n    b();\n}\nfn bar() {\n    c();\n}\n";
    let mut app = AppBuilder::new().with_input_text(input).build()?;

    test_key_sequences(
        &mut app,
        vec![(
            Some("zM"),
            Some(&|app| {
                let (view, doc) = helix_view::current_ref!(app.editor);
                let folds = doc.collapsed_folds(view.id).unwrap_or(&[]);
                assert_eq!(folds.len(), 2, "both blocks should be collapsed");
            }),
        )],
        false,
    )
    .await?;

    Ok(())
}
