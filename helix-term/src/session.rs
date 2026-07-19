//! VSCode-like per-workspace session persistence.
//!
//! A session captures, for the current workspace:
//! - the open file-backed buffers,
//! - the split layout and which document each view shows,
//! - every view's selections (cursor) and scroll position,
//! - the file explorer sidebar state (expanded folders, selection, scroll).
//!
//! Sessions are stored as TOML files, one per workspace (plus an optional
//! session name), under the session directory (default
//! `~/.cache/helix/sessions`). Without an explicit session name, opening a
//! project restores its most recently updated session. The directory and
//! the session name can be configured with the `--session-dir`/`--session`
//! command line flags and the `HELIX_SESSION_DIR`/`HELIX_SESSION`
//! environment variables.

use std::{
    hash::Hasher,
    io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use helix_core::{coords_at_pos, pos_at_coords, Position, Range, Selection, SmallVec};
use helix_view::{
    editor::Action,
    tree::{Layout, SplitTree},
    view::ViewPosition,
    DocumentId, Editor, ViewId,
};
use serde::{Deserialize, Serialize};

use crate::{
    args::{Args, SessionArg},
    ui::FileTree,
};

const SESSION_VERSION: u32 = 1;
/// Minimum interval between two automatic session writes. Saving is
/// triggered on redraw whenever the session changed, so this throttles
/// writes while editing continuously. `SessionManager::save` bypasses the
/// throttle for the final write on exit.
const SAVE_INTERVAL: Duration = Duration::from_secs(1);

/// A serializable cursor position (0-based row/column), mirroring
/// [`helix_core::Position`] which does not implement serde traits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Pos {
    pub row: usize,
    pub col: usize,
}

impl From<Position> for Pos {
    fn from(pos: Position) -> Self {
        Self {
            row: pos.row,
            col: pos.col,
        }
    }
}

impl From<Pos> for Position {
    fn from(pos: Pos) -> Self {
        Position::new(pos.row, pos.col)
    }
}

/// One end-point of a selection range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RangeState {
    pub anchor: Pos,
    pub head: Pos,
}

/// The persisted state of a single view (a leaf of the split tree).
#[derive(Debug, Serialize, Deserialize)]
pub struct ViewState {
    pub path: PathBuf,
    #[serde(default)]
    pub selections: Vec<RangeState>,
    #[serde(default)]
    pub primary: usize,
    pub scroll_anchor: Option<Pos>,
    #[serde(default)]
    pub vertical_offset: usize,
    #[serde(default)]
    pub horizontal_offset: usize,
}

/// The persisted split layout. Views that showed a document without a path
/// (scratch buffers) are not persisted.
#[derive(Debug, Serialize, Deserialize)]
pub enum LayoutState {
    View(ViewState),
    Container { layout: Layout, children: Vec<LayoutState> },
}

/// A buffer that was open when the session was saved.
#[derive(Debug, Serialize, Deserialize)]
pub struct BufferState {
    pub path: PathBuf,
    /// Primary cursor position, restored only when the buffer is not shown
    /// in any view (views store their own selections).
    pub cursor: Option<Pos>,
}

/// The persisted file explorer sidebar state.
#[derive(Debug, Serialize, Deserialize)]
pub struct SidebarState {
    pub root: PathBuf,
    #[serde(default)]
    pub expanded: Vec<PathBuf>,
    pub selected: Option<PathBuf>,
    #[serde(default)]
    pub scroll: usize,
}

/// The full persisted session of a workspace.
#[derive(Debug, Serialize, Deserialize)]
pub struct Session {
    pub version: u32,
    pub workspace: PathBuf,
    #[serde(default)]
    pub buffers: Vec<BufferState>,
    pub layout: Option<LayoutState>,
    /// Index of the focused view among the layout leaves, in depth-first
    /// pre-order (the same order [`helix_view::tree::Tree::traverse`] yields
    /// views).
    #[serde(default)]
    pub focused_view: usize,
    pub sidebar: Option<SidebarState>,
}

/// Static configuration of the session system, resolved once at startup
/// from command line arguments and environment variables.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// The session name; `None` selects the default per-workspace session.
    pub name: Option<String>,
    /// Directory where session files are stored.
    pub dir: PathBuf,
}

impl SessionConfig {
    /// Resolves the session configuration. Returns `None` when sessions are
    /// disabled. Command line arguments take precedence over environment
    /// variables.
    pub fn resolve(args: &Args) -> Option<Self> {
        let name = match &args.session {
            SessionArg::Disabled => return None,
            SessionArg::Enabled(name) => name.clone(),
            SessionArg::Unspecified => match std::env::var("HELIX_SESSION") {
                Ok(value) => match value.to_ascii_lowercase().as_str() {
                    "0" | "false" | "off" | "no" => return None,
                    // An explicit name is used verbatim; the usual truthy
                    // values select the default session.
                    "" | "1" | "true" | "on" | "yes" => None,
                    _ => Some(value),
                },
                Err(_) => None,
            },
        };

        let dir = args
            .session_dir
            .clone()
            .or_else(|| std::env::var_os("HELIX_SESSION_DIR").map(PathBuf::from))
            .unwrap_or_else(|| helix_loader::cache_dir().join("sessions"));

        Some(Self {
            name: name.map(sanitize_name),
            dir,
        })
    }

    /// The session file for the given workspace root.
    pub fn file_for(&self, workspace: &Path) -> PathBuf {
        let mut file_name = format!("{:016x}", fnv1a(workspace.to_string_lossy().as_bytes()));
        if let Some(name) = &self.name {
            file_name.push('.');
            file_name.push_str(name);
        }
        file_name.push_str(".toml");
        self.dir.join(file_name)
    }
}

/// Finds the most recently modified session file of a workspace: the
/// default session `<hash>.toml` or any named session `<hash>.<name>.toml`.
fn latest_session_file(config: &SessionConfig, workspace: &Path) -> Option<PathBuf> {
    let hash = format!("{:016x}", fnv1a(workspace.to_string_lossy().as_bytes()));
    std::fs::read_dir(&config.dir)
        .ok()?
        .flatten()
        .filter(|entry| {
            let Some(file_name) = entry.file_name().to_str().map(str::to_owned) else {
                return false;
            };
            file_name
                .strip_prefix(&hash)
                .is_some_and(|rest| rest == ".toml" || (rest.starts_with('.') && rest.ends_with(".toml")))
        })
        .max_by_key(|entry| entry.metadata().and_then(|m| m.modified()).ok())
        .map(|entry| entry.path())
}

/// Keeps only characters that are safe in a file name.
fn sanitize_name(name: String) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect()
}

/// FNV-1a, used as a stable key for workspace paths (unlike
/// [`std::collections::hash_map::DefaultHasher`], the result is guaranteed
/// not to change between runs).
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Manages loading and debounced saving of the session of the current
/// workspace.
pub struct SessionManager {
    workspace: PathBuf,
    file: PathBuf,
    last_signature: u64,
    last_write: Instant,
}

impl SessionManager {
    pub fn new(config: &SessionConfig) -> Self {
        let workspace = helix_loader::find_workspace().0;
        // Without an explicit session name, open the project's most recently
        // updated session (the default one or any named one); saving then
        // goes back to that same file, keeping it the most recent one.
        let file = match &config.name {
            Some(_) => config.file_for(&workspace),
            None => latest_session_file(config, &workspace)
                .unwrap_or_else(|| config.file_for(&workspace)),
        };
        Self {
            file,
            workspace,
            last_signature: 0,
            last_write: Instant::now(),
        }
    }

    pub fn file(&self) -> &Path {
        &self.file
    }

    /// Loads the session from disk, if a session file exists and parses.
    pub fn load(&self) -> Option<Session> {
        let contents = std::fs::read_to_string(&self.file).ok()?;
        let mut session: Session = match toml::from_str(&contents) {
            Ok(session) => session,
            Err(err) => {
                log::warn!("failed to parse session file {}: {err}", self.file.display());
                return None;
            }
        };
        session.prune_missing();
        Some(session)
    }

    /// Saves the session if it changed since the last write, throttled to
    /// at most one write per [`SAVE_INTERVAL`].
    pub fn save_if_changed(&mut self, editor: &Editor, sidebar: Option<&FileTree>) {
        if self.last_write.elapsed() < SAVE_INTERVAL {
            return;
        }
        self.save_debounced(editor, sidebar);
    }

    /// Saves the session unconditionally (used on exit).
    pub fn save(&mut self, editor: &Editor, sidebar: Option<&FileTree>) {
        self.last_write = Instant::now() - SAVE_INTERVAL;
        self.save_debounced(editor, sidebar);
    }

    fn save_debounced(&mut self, editor: &Editor, sidebar: Option<&FileTree>) {
        let Some(session) = snapshot(&self.workspace, editor, sidebar) else {
            return;
        };
        let Ok(contents) = toml::to_string_pretty(&session) else {
            return;
        };
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        hasher.write(contents.as_bytes());
        let signature = hasher.finish();
        if signature == self.last_signature {
            return;
        }
        match write(&self.file, &contents) {
            Ok(()) => {
                self.last_signature = signature;
                self.last_write = Instant::now();
            }
            Err(err) => {
                log::warn!("failed to write session file {}: {err}", self.file.display());
                // Do not retry on every redraw.
                self.last_write = Instant::now();
            }
        }
    }
}

fn write(file: &Path, contents: &str) -> io::Result<()> {
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(file, contents)
}

/// Builds a [`Session`] from the current editor and sidebar state. Returns
/// `None` when there is nothing worth persisting (no file-backed buffer).
pub fn snapshot(workspace: &Path, editor: &Editor, sidebar: Option<&FileTree>) -> Option<Session> {
    let buffers: Vec<BufferState> = editor
        .documents()
        .filter_map(|doc| {
            let path = doc.path()?.to_path_buf();
            let cursor = doc
                .selections()
                .values()
                .next()
                .map(|sel| coords_at_pos(doc.text().slice(..), sel.primary().head).into());
            Some(BufferState { path, cursor })
        })
        .collect();

    if buffers.is_empty() {
        return None;
    }

    let mut views = editor.tree.traverse();
    let mut leaf_index = 0;
    let mut focused_view = 0;
    let layout = editor
        .tree
        .splits()
        .and_then(|splits| {
            layout_state(
                editor,
                &splits,
                &mut views,
                &mut leaf_index,
                &mut focused_view,
            )
        });

    Some(Session {
        version: SESSION_VERSION,
        workspace: workspace.to_path_buf(),
        buffers,
        layout,
        focused_view,
        sidebar: sidebar.map(sidebar_state),
    })
}

fn layout_state<'a>(
    editor: &Editor,
    node: &SplitTree,
    views: &mut impl Iterator<Item = (ViewId, &'a helix_view::View)>,
    leaf_index: &mut usize,
    focused_view: &mut usize,
) -> Option<LayoutState> {
    match node {
        SplitTree::View => {
            let (view_id, view) = views.next()?;
            let doc = editor.document(view.doc)?;
            let path = doc.path()?.to_path_buf();
            if view_id == editor.tree.focus {
                *focused_view = *leaf_index;
            }
            *leaf_index += 1;
            let text = doc.text().slice(..);
            let selection = doc.selection(view_id);
            let offset = doc.view_offset(view_id);
            Some(LayoutState::View(ViewState {
                path,
                selections: selection
                    .ranges()
                    .iter()
                    .map(|range| RangeState {
                        anchor: coords_at_pos(text, range.anchor).into(),
                        head: coords_at_pos(text, range.head).into(),
                    })
                    .collect(),
                primary: selection.primary_index(),
                scroll_anchor: Some(coords_at_pos(text, offset.anchor).into()),
                vertical_offset: offset.vertical_offset,
                horizontal_offset: offset.horizontal_offset,
            }))
        }
        SplitTree::Container { layout, children } => {
            let children: Vec<LayoutState> = children
                .iter()
                .filter_map(|child| layout_state(editor, child, views, leaf_index, focused_view))
                .collect();
            (!children.is_empty()).then_some(LayoutState::Container {
                layout: *layout,
                children,
            })
        }
    }
}

fn sidebar_state(sidebar: &FileTree) -> SidebarState {
    SidebarState {
        root: sidebar.root.clone(),
        expanded: sidebar
            .entries
            .iter()
            .filter(|entry| entry.is_dir && entry.expanded)
            .map(|entry| entry.path.clone())
            .collect(),
        selected: sidebar
            .entries
            .get(sidebar.selected)
            .map(|entry| entry.path.clone()),
        scroll: sidebar.scroll,
    }
}

impl Session {
    /// Drops buffers and views whose file no longer exists, so that
    /// restoring never fails halfway through the layout.
    fn prune_missing(&mut self) {
        self.buffers.retain(|buffer| buffer.path.is_file());
        if let Some(layout) = &mut self.layout {
            if prune_layout(layout).is_none() {
                self.layout = None;
            }
        }
    }
}

fn prune_layout(node: &mut LayoutState) -> Option<()> {
    match node {
        LayoutState::View(view) => view.path.is_file().then_some(()),
        LayoutState::Container { children, .. } => {
            children.retain_mut(|child| prune_layout(child).is_some());
            (!children.is_empty()).then_some(())
        }
    }
}

/// Restores a session into the editor: opens all buffers, rebuilds the
/// split layout and restores selections and scroll positions. Returns
/// `false` when there was nothing to restore, in which case the caller
/// should fall back to opening an empty buffer.
pub fn restore(editor: &mut Editor, session: &Session) -> bool {
    if session.buffers.is_empty() && session.layout.is_none() {
        return false;
    }

    // Ensure there is a view to open buffers into; it is replaced (and the
    // scratch buffer removed) when the first view of the layout is restored.
    editor.new_file(Action::VerticalSplit);

    for buffer in &session.buffers {
        open_doc(editor, &buffer.path);
    }

    let mut leaves: Vec<(ViewId, &ViewState)> = Vec::new();
    if let Some(layout) = &session.layout {
        restore_layout(editor, layout, &mut leaves);
    }

    // Restore selections and scroll positions for every restored view.
    for (view_id, view_state) in &leaves {
        apply_view_state(editor, *view_id, view_state);
    }

    // Restore the primary cursor of buffers that are not shown in any view.
    let shown: Vec<ViewId> = leaves.iter().map(|(view_id, _)| *view_id).collect();
    for buffer in &session.buffers {
        let Some(cursor) = buffer.cursor else { continue };
        let Some(doc_id) = editor.document_id_by_path(&buffer.path) else {
            continue;
        };
        let Some(doc) = editor.document_mut(doc_id) else {
            continue;
        };
        // Buffers opened with `Action::Load` share the view that was focused
        // at load time; if none of the restored views reference the buffer,
        // update its selection in whatever view it was initialized with.
        let referenced = shown
            .iter()
            .any(|view_id| doc.selections().contains_key(view_id));
        if !referenced {
            if let Some(view_id) = doc.selections().keys().next().copied() {
                let pos = pos_at_coords(doc.text().slice(..), cursor.into(), true);
                doc.set_selection(view_id, Selection::point(pos));
            }
        }
    }

    // Focus the previously focused view.
    if let Some((view_id, _)) = leaves.get(session.focused_view).or(leaves.last()) {
        editor.tree.focus = *view_id;
        let (view, doc) = current!(editor);
        helix_view::align_view(doc, view, helix_view::Align::Center);
    }

    !leaves.is_empty()
}

/// Rebuilds the split tree. Because [`helix_view::tree::Tree::split`]
/// always splits the focused view and may re-wrap it in a new container,
/// all sibling anchors of a container are created up front (before any
/// child subtree is filled in), which reproduces the saved structure
/// exactly.
fn restore_layout<'a>(
    editor: &mut Editor,
    node: &'a LayoutState,
    leaves: &mut Vec<(ViewId, &'a ViewState)>,
) {
    match node {
        LayoutState::View(view_state) => {
            let view_id = editor.tree.focus;
            if let Some(doc_id) = open_doc(editor, &view_state.path) {
                editor.switch(doc_id, Action::Replace);
                leaves.push((view_id, view_state));
            }
        }
        LayoutState::Container { layout, children } => {
            let action = match layout {
                Layout::Vertical => Action::VerticalSplit,
                Layout::Horizontal => Action::HorizontalSplit,
            };
            // Create one anchor view per child, splitting after the previous
            // anchor so that all anchors end up as siblings in one container
            // of the right layout.
            let mut anchors: Vec<Option<ViewId>> = Vec::with_capacity(children.len());
            let mut last_anchor = Some(editor.tree.focus);
            anchors.push(last_anchor);
            for child in &children[1..] {
                let anchor = first_leaf_path(child)
                    .and_then(|path| open_doc(editor, path))
                    .and_then(|doc_id| {
                        editor.tree.focus = last_anchor?;
                        editor.switch(doc_id, action);
                        Some(editor.tree.focus)
                    });
                if anchor.is_some() {
                    last_anchor = anchor;
                }
                anchors.push(anchor);
            }
            // Fill in each child subtree in its anchor view.
            for (child, anchor) in children.iter().zip(anchors) {
                if let Some(anchor) = anchor {
                    editor.tree.focus = anchor;
                    restore_layout(editor, child, leaves);
                }
            }
        }
    }
}

fn first_leaf_path(node: &LayoutState) -> Option<&Path> {
    match node {
        LayoutState::View(view_state) => Some(&view_state.path),
        LayoutState::Container { children, .. } => children.iter().find_map(first_leaf_path),
    }
}

fn open_doc(editor: &mut Editor, path: &Path) -> Option<DocumentId> {
    match editor.open(path, Action::Load) {
        Ok(doc_id) => Some(doc_id),
        Err(err) => {
            log::warn!("session: failed to open {}: {err}", path.display());
            None
        }
    }
}

fn apply_view_state(editor: &mut Editor, view_id: ViewId, view_state: &ViewState) {
    let doc_id = editor.tree.get(view_id).doc;
    let Some(doc) = editor.document_mut(doc_id) else {
        return;
    };
    let text = doc.text().slice(..);
    let anchor = view_state
        .scroll_anchor
        .map(|pos| pos_at_coords(text, pos.into(), true))
        .unwrap_or(0);
    if !view_state.selections.is_empty() {
        let ranges: SmallVec<[Range; 1]> = view_state
            .selections
            .iter()
            .map(|range| {
                Range::new(
                    pos_at_coords(text, range.anchor.into(), true),
                    pos_at_coords(text, range.head.into(), true),
                )
            })
            .collect();
        let primary = view_state.primary.min(ranges.len() - 1);
        doc.set_selection(view_id, Selection::new(ranges, primary));
    }
    doc.set_view_offset(
        view_id,
        ViewPosition {
            anchor,
            horizontal_offset: view_state.horizontal_offset,
            vertical_offset: view_state.vertical_offset,
        },
    );
}

/// Rebuilds the file explorer sidebar from its persisted state.
pub fn restore_sidebar(state: &SidebarState, editor: &Editor) -> Option<FileTree> {
    if !state.root.is_dir() {
        return None;
    }
    let mut tree = FileTree::new(state.root.clone(), editor);
    // Expand parents before their children.
    let mut expanded = state.expanded.clone();
    expanded.sort_by_key(|path| path.components().count());
    for path in expanded {
        if let Some(idx) = tree
            .entries
            .iter()
            .position(|entry| entry.is_dir && !entry.expanded && entry.path == path)
        {
            tree.toggle_expand(idx);
        }
    }
    if let Some(selected) = &state.selected {
        if let Some(idx) = tree.entries.iter().position(|entry| &entry.path == selected) {
            tree.selected = idx;
        }
    }
    tree.scroll = state.scroll;
    Some(tree)
}
