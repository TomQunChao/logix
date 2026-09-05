use helix_event::register_hook;
use helix_view::{
    editor::Config,
    events::{DocumentDidOpen, DocumentFocusLost},
    Editor,
};

use crate::handlers::Handlers;

const APP_NAME: &str = "logix";
const SCRATCH_SUMMARY: &str = "[scratch]";

fn build_title(editor: &Editor, config: &Config) -> String {
    let sep = &config.terminal_title_separator;

    let doc_id = editor.tree.get(editor.tree.focus).doc;

    let doc = match editor.documents.get(&doc_id) {
        Some(doc) => doc,
        None => return APP_NAME.to_string(),
    };

    let project = doc
        .workspace_root()
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| APP_NAME.to_string());

    let file = doc
        .relative_path()
        .map(|p| p.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| SCRATCH_SUMMARY.to_string());

    format!("{APP_NAME}{sep}{project}{sep}{file}")
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
