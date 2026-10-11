//! Regression tests for tree-sitter textobjects.
//!
//! Previously `mif`/`mit` (and other `*i*` textobjects) did nothing when the
//! cursor was on the item's keyword (`fn`, `struct`, ...), because the cursor is
//! within the `*.around` node but not within the `*.inside` node.

use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn function_textobject_on_keyword() -> anyhow::Result<()> {
    test_with_config(
        AppBuilder::new().with_file("foo.rs", None),
        (
            "#[|]#fn foo() {\n    bar();\n}\n",
            "mif",
            "fn foo() #[{\n    bar();\n}|]#\n",
        ),
    )
    .await?;

    test_with_config(
        AppBuilder::new().with_file("foo.rs", None),
        (
            "#[|]#fn foo() {\n    bar();\n}\n",
            "maf",
            "#[fn foo() {\n    bar();\n}|]#\n",
        ),
    )
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn type_textobject_on_keyword() -> anyhow::Result<()> {
    test_with_config(
        AppBuilder::new().with_file("foo.rs", None),
        (
            "#[|]#struct Foo { x: u32 }\n",
            "mit",
            "struct Foo #[{ x: u32 }|]#\n",
        ),
    )
    .await?;

    test_with_config(
        AppBuilder::new().with_file("foo.rs", None),
        (
            "#[|]#struct Foo { x: u32 }\n",
            "mat",
            "#[struct Foo { x: u32 }|]#\n",
        ),
    )
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn textobject_inside_when_cursor_is_in_body() -> anyhow::Result<()> {
    test_with_config(
        AppBuilder::new().with_file("foo.rs", None),
        (
            "fn foo() {\n    #[|]#bar();\n}\n",
            "mif",
            "fn foo() #[{\n    bar();\n}|]#\n",
        ),
    )
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn textobject_around_at_end_of_file() -> anyhow::Result<()> {
    // The item ends exactly at EOF (no trailing newline): the selection must still work.
    test_with_config(
        AppBuilder::new().with_file("foo.rs", None),
        (
            "#[|]#fn foo() { bar(); }",
            "maf",
            "#[fn foo() { bar(); }|]#",
            LineFeedHandling::AsIs,
        ),
    )
    .await?;

    Ok(())
}
