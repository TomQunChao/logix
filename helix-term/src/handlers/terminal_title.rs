use std::path::Path;

use helix_event::register_hook;
use helix_view::{
    editor::Config,
    events::{DocumentDidOpen, DocumentFocusLost},
    Editor,
};

use crate::handlers::Handlers;

const APP_NAME: &str = "lx";
const SCRATCH_SUMMARY: &str = "[scratch]";
const ELLIPSIS: char = '…';

fn build_title(editor: &Editor, config: &Config) -> String {
    let sep = &config.terminal_title_separator;

    // The focused view may not be resolvable yet (e.g. while a document is being opened).
    let Some(view) = editor.tree.try_get(editor.tree.focus) else {
        return APP_NAME.to_string();
    };

    let doc = match editor.documents.get(&view.doc) {
        Some(doc) => doc,
        None => return APP_NAME.to_string(),
    };

    // The current file's name, or the workspace's name when the buffer has no file.
    let name = doc
        .path()
        .and_then(Path::file_name)
        .or_else(|| doc.workspace_root().file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| SCRATCH_SUMMARY.to_string());

    compose_title(&name, sep, title_max_width(editor, config))
}

/// Compose the title from `name`, truncating it to at most `max_width` characters.
fn compose_title(name: &str, sep: &str, max_width: usize) -> String {
    truncate(format!("{APP_NAME}{sep}{name}"), max_width)
}

/// Maximum title length in characters: the `editor.terminal-title-max-width` override when
/// set, otherwise the terminal width.
fn title_max_width(editor: &Editor, config: &Config) -> usize {
    if let Some(max_width) = config.terminal_title_max_width {
        return max_width;
    }
    // `tree.area()` is the terminal area laid out on render; a zero width means the editor
    // has not been laid out yet, so leave the title untruncated.
    match editor.tree.area().width {
        0 => usize::MAX,
        width => width as usize,
    }
}

/// Truncate `title` to at most `max_width` characters, replacing the tail with an ellipsis.
fn truncate(mut title: String, max_width: usize) -> String {
    if title.chars().count() <= max_width {
        return title;
    }
    if max_width == 0 {
        return String::new();
    }
    title = title.chars().take(max_width - 1).collect();
    title.push(ELLIPSIS);
    title
}

#[cfg(not(feature = "integration"))]
fn write_title(title: &str) {
    use std::io::{self, Write};
    let mut out = io::stdout();
    let _ = write!(out, "\x1b]0;{title}\x07");
    let _ = out.flush();
}

#[cfg(feature = "integration")]
fn write_title(_title: &str) {}

#[cfg(not(feature = "integration"))]
pub fn update(editor: &Editor) {
    let title = build_title(editor, &editor.config());
    write_title(&title);
}

#[cfg(feature = "integration")]
pub fn update(_editor: &Editor) {}

pub(super) fn register_hooks(_handlers: &Handlers) {
    register_hook!(move |event: &mut DocumentDidOpen<'_>| {
        let title = build_title(event.editor, &event.editor.config());
        write_title(&title);
        Ok(())
    });

    register_hook!(move |event: &mut DocumentFocusLost<'_>| {
        let title = build_title(event.editor, &event.editor.config());
        write_title(&title);
        Ok(())
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_is_app_name_separator_and_name() {
        assert_eq!(compose_title("main.rs", " - ", 80), "lx - main.rs");
        assert_eq!(compose_title("logix", " - ", 80), "lx - logix");
        assert_eq!(compose_title("main.rs", " | ", 80), "lx | main.rs");
    }

    #[test]
    fn title_fitting_the_width_is_left_intact() {
        let title = compose_title("main.rs", " - ", 80);
        let exact = compose_title("main.rs", " - ", title.chars().count());
        assert_eq!(title, "lx - main.rs");
        assert_eq!(exact, title);
    }

    #[test]
    fn long_title_is_ellipsized_to_the_max_width() {
        let title = compose_title("a-very-long-file-name.rs", " - ", 10);
        assert_eq!(title, "lx - a-ve…");
        assert_eq!(title.chars().count(), 10);
    }

    #[test]
    fn tiny_and_zero_widths_are_handled() {
        assert_eq!(compose_title("file.rs", " - ", 1), "…");
        assert_eq!(compose_title("file.rs", " - ", 0), "");
    }

    #[test]
    fn truncation_counts_characters_not_bytes() {
        // Multi-byte characters are counted one each and never split.
        let title = compose_title("日本語のファイル.rs", " - ", 5);
        assert_eq!(title.chars().count(), 5);
        assert!(title.ends_with('…'));
        assert!(title.starts_with("lx -"));
    }
}
