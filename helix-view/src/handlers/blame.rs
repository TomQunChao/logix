//! Keeps the inline git blame annotation in sync with the cursor line.

use helix_event::register_hook;

use crate::events::SelectionDidChange;

pub(super) fn register_hooks() {
    register_hook!(move |event: &mut SelectionDidChange<'_>| {
        event.doc.refresh_blame_annotation(event.view);
        Ok(())
    });
}
