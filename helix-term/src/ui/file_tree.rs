use crate::compositor::{Component, Context, Event, EventResult};
use helix_core::unicode::width::UnicodeWidthStr;
use helix_view::{
    editor::Action,
    graphics::{CursorKind, Rect},
    input::KeyEvent,
    keyboard::{KeyCode, KeyModifiers},
    Editor,
};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tui::{
    buffer::Buffer as Surface,
    text::{Span, Spans},
};

use helix_core::Position;

/// Git status for a file
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitStatus {
    Untracked,
    Modified,
    Added,
    Deleted,
    Renamed,
    Conflict,
}

impl GitStatus {
    fn label(&self) -> &'static str {
        match self {
            Self::Untracked => "?",
            Self::Modified => "M",
            Self::Added => "A",
            Self::Deleted => "D",
            Self::Renamed => "R",
            Self::Conflict => "C",
        }
    }
}

/// A single entry in the file tree
#[derive(Debug, Clone)]
pub struct FileEntry {
    /// Full path to the file/directory
    pub path: PathBuf,
    /// Whether this is a directory
    pub is_dir: bool,
    /// Depth in the tree (0 = root level)
    pub depth: usize,
    /// Whether this directory is expanded (showing children)
    pub expanded: bool,
    /// Git status (if available)
    pub git_status: Option<GitStatus>,
    /// Display name (file/directory name only)
    pub name: String,
}

impl FileEntry {
    fn new(path: PathBuf, is_dir: bool, depth: usize) -> Self {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self {
            path,
            is_dir,
            depth,
            expanded: false,
            git_status: None,
            name,
        }
    }
}

/// The file tree sidebar component
pub struct FileTree {
    /// Root directory of the workspace
    pub root: PathBuf,
    /// All entries in the tree (flattened)
    pub entries: Vec<FileEntry>,
    /// Currently selected entry index
    pub selected: usize,
    /// Scroll offset for the visible area
    pub scroll: usize,
    /// Whether we're in filter mode
    pub filter_mode: bool,
    /// The filter query string
    pub filter_query: String,
    /// Whether to show git status
    pub show_git_status: bool,
    /// Cached git status for files
    pub git_status: HashMap<PathBuf, GitStatus>,
    /// Maximum width as percentage of total width
    pub max_width_percent: u8,
    /// Minimum width in columns
    pub min_width: u16,
    /// Cached height for scroll calculations
    cached_height: u16,
}

impl FileTree {
    pub fn new(root: PathBuf, editor: &Editor) -> Self {
        let mut tree = Self {
            root,
            entries: Vec::new(),
            selected: 0,
            scroll: 0,
            filter_mode: false,
            filter_query: String::new(),
            show_git_status: true,
            git_status: HashMap::new(),
            max_width_percent: 35,
            min_width: 25,
            cached_height: 20,
        };
        tree.load_directory(&tree.root.clone(), 0);
        tree.load_git_status(editor);
        tree
    }

    /// Load directory contents at the given path and depth
    fn load_directory(&mut self, path: &Path, depth: usize) {
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            Err(_) => return,
        };

        let mut dirs: Vec<FileEntry> = Vec::new();
        let mut files: Vec<FileEntry> = Vec::new();

        for entry in entries.flatten() {
            let path = entry.path();
            let is_dir = path.is_dir();
            let mut file_entry = FileEntry::new(path, is_dir, depth);

            // Apply git status if available
            file_entry.git_status = self.git_status.get(&file_entry.path).copied();

            if is_dir {
                dirs.push(file_entry);
            } else {
                files.push(file_entry);
            }
        }

        // Sort: directories first, then files, both alphabetically
        dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

        // Find insertion point
        let insert_idx = if depth == 0 {
            0
        } else {
            // Find the parent entry and insert after it
            self.entries
                .iter()
                .position(|e| e.path == *path)
                .map(|p| p + 1)
                .unwrap_or(self.entries.len())
        };

        // Insert entries
        let all_entries: Vec<FileEntry> = dirs.into_iter().chain(files.into_iter()).collect();
        for (i, entry) in all_entries.into_iter().enumerate() {
            self.entries.insert(insert_idx + i, entry);
        }
    }

    /// Load git status for all files in the workspace
    fn load_git_status(&mut self, editor: &Editor) {
        // Use the diff_providers to get changed files
        let root = self.root.clone();
        let git_status = Arc::new(Mutex::new(HashMap::new()));

        let trust_full = editor
            .workspace_trust
            .query(
                &helix_loader::find_workspace_in(&root).0,
                helix_loader::workspace_trust::TrustQuery::Git,
            )
            .is_trusted();

        let git_status_clone = git_status.clone();
        editor
            .diff_providers
            .clone()
            .for_each_changed_file(root, trust_full, move |change| {
                use helix_vcs::FileChange;
                match change {
                    Ok(change) => {
                        let status = match change {
                            FileChange::Untracked { .. } => GitStatus::Untracked,
                            FileChange::Modified { .. } => GitStatus::Modified,
                            FileChange::Conflict { .. } => GitStatus::Conflict,
                            FileChange::Deleted { .. } => GitStatus::Deleted,
                            FileChange::Renamed { .. } => GitStatus::Renamed,
                        };
                        if let Ok(mut map) = git_status_clone.lock() {
                            map.insert(change.path().to_path_buf(), status);
                        }
                    }
                    Err(_) => return false,
                }
                true
            });

        // Collect the results
        if let Ok(map) = git_status.lock() {
            self.git_status = map.clone();
        };
    }

    /// Refresh the file tree
    pub fn refresh(&mut self, editor: &Editor) {
        self.entries.clear();
        self.git_status.clear();
        self.load_directory(&self.root.clone(), 0);
        self.load_git_status(editor);
        if self.selected >= self.entries.len() && !self.entries.is_empty() {
            self.selected = self.entries.len() - 1;
        }
    }

    /// Compute the width based on content
    pub fn compute_width(&self, max_width: u16) -> u16 {
        let content_width = self
            .entries
            .iter()
            .skip(self.scroll)
            .take(max_width as usize)
            .map(|e| {
                // indentation + icon + name + git status
                let indent = e.depth * 2;
                let icon = if e.is_dir { 2 } else { 1 }; // "▸ " or "  "
                let name = e.name.width();
                let git = if e.git_status.is_some() { 2 } else { 0 };
                indent + icon + name + git + 2 // +2 for padding
            })
            .max()
            .unwrap_or(20) as u16;

        let max_allowed = (max_width as u32 * self.max_width_percent as u32 / 100) as u16;
        content_width.clamp(self.min_width, max_allowed.max(self.min_width))
    }

    /// Toggle expansion of a directory
    fn toggle_expand(&mut self, idx: usize) {
        if idx >= self.entries.len() {
            return;
        }

        let entry = &self.entries[idx];
        if !entry.is_dir {
            return;
        }

        let path = entry.path.clone();
        let is_expanded = entry.expanded;

        if is_expanded {
            // Collapse: remove all children
            let child_depth = entry.depth + 1;
            let mut remove_count = 0;
            for i in (idx + 1)..self.entries.len() {
                if self.entries[i].depth >= child_depth {
                    remove_count += 1;
                } else {
                    break;
                }
            }
            self.entries.drain((idx + 1)..(idx + 1 + remove_count));
            self.entries[idx].expanded = false;
        } else {
            // Expand: load children
            let depth = entry.depth + 1;
            self.load_directory_children(&path, depth, idx + 1);
            self.entries[idx].expanded = true;
        }
    }

    /// Load children of a directory at a specific position
    fn load_directory_children(&mut self, path: &Path, depth: usize, insert_idx: usize) {
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            Err(_) => return,
        };

        let mut dirs: Vec<FileEntry> = Vec::new();
        let mut files: Vec<FileEntry> = Vec::new();

        for entry in entries.flatten() {
            let path = entry.path();
            let is_dir = path.is_dir();
            let mut file_entry = FileEntry::new(path, is_dir, depth);
            file_entry.git_status = self.git_status.get(&file_entry.path).copied();

            if is_dir {
                dirs.push(file_entry);
            } else {
                files.push(file_entry);
            }
        }

        dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

        let all_entries: Vec<FileEntry> = dirs.into_iter().chain(files.into_iter()).collect();
        for (i, entry) in all_entries.into_iter().enumerate() {
            self.entries.insert(insert_idx + i, entry);
        }
    }

    /// Open the selected file or toggle directory expansion
    fn open_selected(&mut self, cx: &mut Context) {
        if self.selected >= self.entries.len() {
            return;
        }

        let entry = &self.entries[self.selected];
        if entry.is_dir {
            self.toggle_expand(self.selected);
        } else {
            // Open the file
            let path = entry.path.clone();
            if let Err(e) = cx.editor.open(&path, Action::Replace) {
                cx.editor.set_error(format!("Failed to open file: {:?}", e));
            }
        }
    }

    /// Move selection up
    fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Move selection down
    fn move_down(&mut self) {
        if self.selected + 1 < self.entries.len() {
            self.selected += 1;
        }
    }

    /// Update scroll to ensure selected is visible
    fn update_scroll(&mut self) {
        let visible_height = self.cached_height as usize;
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + visible_height.saturating_sub(1) {
            self.scroll = self.selected - visible_height.saturating_sub(2);
        }
    }

    /// Create a new file or directory at the current selection
    #[allow(dead_code)]
    fn create_item(&mut self, cx: &mut Context, name: &str) {
        if name.is_empty() {
            return;
        }

        // Determine parent directory
        let parent_dir = if self.selected < self.entries.len() {
            let entry = &self.entries[self.selected];
            if entry.is_dir {
                entry.path.clone()
            } else {
                entry.path.parent().unwrap_or(&self.root).to_path_buf()
            }
        } else {
            self.root.clone()
        };

        let is_dir = name.ends_with('/');
        let name = name.trim_end_matches('/');
        let new_path = parent_dir.join(name);

        let result = if is_dir {
            fs::create_dir_all(&new_path)
        } else {
            // Create parent dirs if needed, then create empty file
            if let Some(parent) = new_path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            fs::File::create(&new_path).map(|_| ())
        };

        match result {
            Ok(()) => {
                cx.editor
                    .set_status(format!("Created: {}", new_path.display()));
                self.refresh(cx.editor);
            }
            Err(e) => {
                cx.editor.set_error(format!("Failed to create: {:?}", e));
            }
        }
    }

    /// Delete the selected item
    fn delete_selected(&mut self, cx: &mut Context) {
        if self.selected >= self.entries.len() {
            return;
        }

        let entry = &self.entries[self.selected];
        let path = entry.path.clone();

        let result = if entry.is_dir {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };

        match result {
            Ok(()) => {
                cx.editor.set_status(format!("Deleted: {}", path.display()));
                self.refresh(cx.editor);
            }
            Err(e) => {
                cx.editor.set_error(format!("Failed to delete: {:?}", e));
            }
        }
    }

    /// Render a single entry
    fn render_entry(
        &self,
        entry: &FileEntry,
        selected: bool,
        area: Rect,
        surface: &mut Surface,
        theme: &helix_view::Theme,
    ) {
        let x = area.x;
        let y = area.y;

        // Background for selected item
        if selected {
            let selected_style = theme.get("ui.selection");
            surface.set_style(area, selected_style);
        }

        // Build the display line
        let mut spans = Vec::new();

        // Indentation
        let indent = " ".repeat(entry.depth * 2);
        if !indent.is_empty() {
            spans.push(Span::raw(indent));
        }

        // Icon
        let icon = if entry.is_dir {
            if entry.expanded {
                "▾ "
            } else {
                "▸ "
            }
        } else {
            "  "
        };
        let dir_style = theme.get("ui.text.directory");
        let text_style = theme.get("ui.text");
        let icon_style = if entry.is_dir { dir_style } else { text_style };
        spans.push(Span::styled(icon, icon_style));

        // Name
        let name_style = if entry.is_dir { dir_style } else { text_style };
        spans.push(Span::styled(&entry.name, name_style));

        // Git status
        if let Some(status) = entry.git_status {
            let git_style = match status {
                GitStatus::Untracked => theme.get("diff.plus"),
                GitStatus::Modified => theme.get("diff.delta"),
                GitStatus::Added => theme.get("diff.plus"),
                GitStatus::Deleted => theme.get("diff.minus"),
                GitStatus::Renamed => theme.get("diff.delta.moved"),
                GitStatus::Conflict => theme.get("error"),
            };
            spans.push(Span::styled(format!(" {}", status.label()), git_style));
        }

        let line = Spans::from(spans);
        surface.set_spans(x, y, &line, area.width);
    }
}

impl Component for FileTree {
    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        // Render as a left sidebar using only the computed width.  This leaves
        // the rest of the viewport untouched so the editor remains visible.
        let tree_width = self.compute_width(area.width);
        let tree_area = Rect::new(area.x, area.y, tree_width, area.height);

        // Cache height for scroll calculations
        self.cached_height = tree_area.height;

        // Background
        let bg_style = cx.editor.theme.get("ui.background");
        surface.set_style(tree_area, bg_style);

        // Border on the right
        let border_style = cx.editor.theme.get("ui.separator");
        for y in tree_area.y..tree_area.y + tree_area.height {
            surface.set_string(tree_area.x + tree_area.width - 1, y, "│", border_style);
        }

        // Title
        let title_style = cx.editor.theme.get("ui.text.focus");
        let title = " File Tree ";
        surface.set_string(tree_area.x + 1, tree_area.y, title, title_style);

        // Render entries
        let entries_area = Rect::new(
            tree_area.x,
            tree_area.y + 1,
            tree_area.width - 1,
            tree_area.height - 1,
        );
        let visible_height = entries_area.height as usize;

        // Update scroll position
        self.update_scroll();

        for (i, entry) in self
            .entries
            .iter()
            .skip(self.scroll)
            .take(visible_height)
            .enumerate()
        {
            let y = entries_area.y + i as u16;
            let row_area = Rect::new(entries_area.x, y, entries_area.width, 1);
            let is_selected = self.scroll + i == self.selected;
            self.render_entry(entry, is_selected, row_area, surface, &cx.editor.theme);
        }

        // Filter mode indicator
        if self.filter_mode {
            let filter_style = cx.editor.theme.get("ui.text");
            let filter_text = format!("/{}", self.filter_query);
            surface.set_string(
                tree_area.x + 1,
                tree_area.y + tree_area.height - 1,
                &filter_text,
                filter_style,
            );
        }

        // Show help hint at bottom if space allows
        if tree_area.height > 3 {
            let help_style = cx.editor.theme.get("ui.text");
            let help = "j/k:nav Enter:open a:create d:del q:close";
            if help.len() < tree_area.width as usize {
                surface.set_string(
                    tree_area.x + 1,
                    tree_area.y + tree_area.height - 1,
                    help,
                    help_style,
                );
            }
        }
    }

    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        if self.filter_mode {
            // Handle filter input
            match event {
                Event::Key(KeyEvent {
                    code: KeyCode::Esc,
                    modifiers: _,
                }) => {
                    self.filter_mode = false;
                    self.filter_query.clear();
                    return EventResult::Consumed(None);
                }
                Event::Key(KeyEvent {
                    code: KeyCode::Enter,
                    modifiers: _,
                }) => {
                    self.filter_mode = false;
                    return EventResult::Consumed(None);
                }
                Event::Key(KeyEvent {
                    code: KeyCode::Char(c),
                    modifiers: KeyModifiers::NONE,
                }) => {
                    self.filter_query.push(*c);
                    // TODO: Apply filter to entries
                    return EventResult::Consumed(None);
                }
                Event::Key(KeyEvent {
                    code: KeyCode::Backspace,
                    modifiers: _,
                }) => {
                    self.filter_query.pop();
                    return EventResult::Consumed(None);
                }
                _ => return EventResult::Ignored(None),
            }
        }

        match event {
            Event::Key(KeyEvent {
                code: KeyCode::Char('c'),
                modifiers: KeyModifiers::CONTROL,
            })
            | Event::Key(KeyEvent {
                code: KeyCode::Esc,
                modifiers: KeyModifiers::NONE,
            }) => EventResult::Consumed(Some(Box::new(|compositor, _cx| {
                compositor.remove("file-tree");
            }))),
            Event::Key(KeyEvent {
                code: KeyCode::Char('q'),
                modifiers: KeyModifiers::NONE,
            }) => EventResult::Consumed(Some(Box::new(|compositor, _cx| {
                compositor.remove("file-tree");
            }))),
            Event::Key(KeyEvent {
                code: KeyCode::Char('j') | KeyCode::Down,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.move_down();
                self.update_scroll();
                EventResult::Consumed(None)
            }
            Event::Key(KeyEvent {
                code: KeyCode::Char('k') | KeyCode::Up,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.move_up();
                self.update_scroll();
                EventResult::Consumed(None)
            }
            Event::Key(KeyEvent {
                code: KeyCode::Char('l') | KeyCode::Enter | KeyCode::Right,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.open_selected(cx);
                EventResult::Consumed(None)
            }
            Event::Key(KeyEvent {
                code: KeyCode::Char('h') | KeyCode::Left,
                modifiers: KeyModifiers::NONE,
            }) => {
                // Collapse current directory or go to parent
                if self.selected < self.entries.len() && self.entries[self.selected].expanded {
                    self.toggle_expand(self.selected);
                }
                EventResult::Consumed(None)
            }
            Event::Key(KeyEvent {
                code: KeyCode::Char('R'),
                modifiers: KeyModifiers::NONE,
            }) => {
                self.refresh(cx.editor);
                EventResult::Consumed(None)
            }
            Event::Key(KeyEvent {
                code: KeyCode::Char('/'),
                modifiers: KeyModifiers::NONE,
            }) => {
                self.filter_mode = true;
                self.filter_query.clear();
                EventResult::Consumed(None)
            }
            Event::Key(KeyEvent {
                code: KeyCode::Char('a'),
                modifiers: KeyModifiers::NONE,
            }) => {
                // Create new file/dir - simple inline input
                // For now, just show a message - full prompt requires more complex handling
                cx.editor
                    .set_status("Create: type filename (a<name> for file, a<name>/ for dir)");
                EventResult::Consumed(None)
            }
            Event::Key(KeyEvent {
                code: KeyCode::Char('d'),
                modifiers: KeyModifiers::NONE,
            }) => {
                // Delete selected item directly (for simplicity, no confirmation)
                self.delete_selected(cx);
                EventResult::Consumed(None)
            }
            Event::Key(KeyEvent {
                code: KeyCode::Char('r'),
                modifiers: KeyModifiers::NONE,
            }) => {
                // Rename - simplified for now
                cx.editor
                    .set_status("Rename: use 'd' to delete and 'a' to create new");
                EventResult::Consumed(None)
            }
            Event::Key(KeyEvent {
                code: KeyCode::Char('g'),
                modifiers: KeyModifiers::NONE,
            }) => {
                // Go to top
                self.selected = 0;
                self.scroll = 0;
                EventResult::Consumed(None)
            }
            Event::Key(KeyEvent {
                code: KeyCode::Char('G'),
                modifiers: KeyModifiers::NONE,
            }) => {
                // Go to bottom
                if !self.entries.is_empty() {
                    self.selected = self.entries.len() - 1;
                    self.update_scroll();
                }
                EventResult::Consumed(None)
            }
            _ => EventResult::Ignored(None),
        }
    }

    fn cursor(&self, _area: Rect, _editor: &Editor) -> (Option<Position>, CursorKind) {
        if self.filter_mode {
            (None, CursorKind::Block)
        } else {
            (None, CursorKind::Hidden)
        }
    }

    fn required_size(&mut self, viewport: (u16, u16)) -> Option<(u16, u16)> {
        let width = self.compute_width(viewport.0);
        Some((width, viewport.1))
    }

    fn id(&self) -> Option<&'static str> {
        Some("file-tree")
    }
}
