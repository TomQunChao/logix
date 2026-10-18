//! Regression tests for `a` (append) at the end of a line.
//!
//! The cursor can rest on the newline grapheme (for example after moving right past the last
//! character). Appending there used to jump into the next line; it should insert at the end
//! of the current line like a standard editor.

use super::*;

async fn assert_after(input: &str, keys: &str, expected: &str) -> anyhow::Result<()> {
    let mut app = AppBuilder::new().with_input_text(input).build()?;
    let expected = expected.to_string();
    let keys = keys.to_string();
    test_key_sequences(
        &mut app,
        vec![(
            Some(&keys),
            Some(&|app| {
                let (_, doc) = helix_view::current_ref!(app.editor);
                assert_eq!(doc.text().to_string(), expected);
            }),
        )],
        false,
    )
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn append_on_last_character_goes_to_end_of_line() -> anyhow::Result<()> {
    assert_after("ab#[|]#c\n", "aX<esc>", "abcX\n").await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn append_on_newline_stays_on_current_line() -> anyhow::Result<()> {
    // Cursor resting on the newline at the end of the buffer.
    assert_after("abc#[|]#\n", "aX<esc>", "abcX\n").await?;
    // Cursor resting on a newline in the middle of the buffer.
    assert_after("abc#[|]#\ndef\n", "aX<esc>", "abcX\ndef\n").await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_on_last_character_inserts_before_it() -> anyhow::Result<()> {
    assert_after("ab#[|]#c\n", "iX<esc>", "abXc\n").await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn goto_line_end_then_append() -> anyhow::Result<()> {
    assert_after("#[|]#abc\n", "glaX<esc>", "abcX\n").await?;
    Ok(())
}
