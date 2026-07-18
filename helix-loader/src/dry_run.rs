//! Dry-run support for the configuration system.
//!
//! When dry-run mode is enabled (via `--dry-run`), operations that would mutate
//! the filesystem or touch the network (creating directories, `git fetch`,
//! invoking compilers, ...) are intercepted: they are *recorded* instead of
//! executed. Read-only probes (checking whether a file exists, reading config
//! files, `git rev-parse`) still run for real so the resulting report reflects
//! the actual state of the system.
//!
//! At the end of a dry-run, [`print_report`] renders a human-readable summary
//! of:
//!
//! * which directories were searched (config dirs, runtime dirs, ...)
//! * which config files were found/read/skipped, layer by layer
//! * which directories *would have been* created
//! * the synthesized (merged) editor and language configuration
//! * which actions (git fetch/build steps) *would have been* performed
//!
//! This is primarily a diagnostic tool to verify that the config system
//! (config dir chain, config file layering, workspace trust gates, grammar
//! resolution) behaves as expected, without touching the system.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;

static ENABLED: AtomicBool = AtomicBool::new(false);
static REPORT: Mutex<Option<Report>> = Mutex::new(None);

/// Enable dry-run mode. Must be called before any of the `initialize_*`
/// functions so their side effects are intercepted.
pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
    *REPORT.lock() = Some(Report::default());
}

/// Whether dry-run mode is active.
pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Outcome of probing/reading a config file layer.
#[derive(Debug, Clone)]
pub enum ReadOutcome {
    /// File existed and parsed successfully.
    Loaded,
    /// File does not exist (layer skipped).
    NotFound,
    /// File exists but could not be read or parsed.
    Error(String),
}

impl fmt::Display for ReadOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Loaded => write!(f, "loaded"),
            Self::NotFound => write!(f, "not found"),
            Self::Error(err) => write!(f, "ERROR: {err}"),
        }
    }
}

/// Summary of the synthesized (merged) language configuration.
#[derive(Debug, Default)]
pub struct LanguagesSummary {
    pub language_count: usize,
    pub grammar_count: usize,
    /// The `use-grammars` selection, if any: `only: [...]` / `except: [...]`.
    pub grammar_selection: Option<String>,
}

#[derive(Debug, Default)]
struct Report {
    created_dirs: Vec<PathBuf>,
    read_configs: Vec<(PathBuf, ReadOutcome)>,
    actions: Vec<(String, String)>,
    /// Rendered TOML of the merged `editor` configuration.
    synthesized_editor: Option<String>,
    /// Theme selected by the merged config.
    synthesized_theme: Option<String>,
    synthesized_languages: Option<LanguagesSummary>,
}

fn with_report(f: impl FnOnce(&mut Report)) {
    let mut guard = REPORT.lock();
    if let Some(report) = guard.as_mut() {
        f(report);
    }
}

/// Record that `create_dir_all(path)` would have been called.
fn record_create_dir(path: PathBuf) {
    with_report(|r| {
        if !r.created_dirs.contains(&path) {
            r.created_dirs.push(path);
        }
    });
}

/// Record the outcome of reading a config file layer.
pub fn record_read_config(path: PathBuf, outcome: ReadOutcome) {
    with_report(|r| r.read_configs.push((path, outcome)));
}

/// Record an action that would have been performed (network, compiler, ...).
pub fn record_action(context: impl Into<String>, detail: impl Into<String>) {
    let (context, detail) = (context.into(), detail.into());
    with_report(|r| r.actions.push((context, detail)));
}

/// Record the synthesized (merged) editor configuration, as pretty TOML.
pub fn record_synthesized_editor(rendered_toml: String) {
    with_report(|r| r.synthesized_editor = Some(rendered_toml));
}

/// Record the theme selected by the merged configuration.
pub fn record_synthesized_theme(theme: String) {
    with_report(|r| r.synthesized_theme = Some(theme));
}

/// Record a summary of the synthesized (merged) language configuration.
pub fn record_synthesized_languages(summary: LanguagesSummary) {
    with_report(|r| r.synthesized_languages = Some(summary));
}

/// Dry-run aware replacement for [`std::fs::create_dir_all`].
///
/// In dry-run mode the directory is *not* created; the attempt is recorded
/// and reported at the end. Otherwise this is a thin wrapper around the real
/// call.
pub fn create_dir_all(path: &Path) -> std::io::Result<()> {
    if is_enabled() {
        if !path.exists() {
            record_create_dir(path.to_path_buf());
        }
        return Ok(());
    }
    std::fs::create_dir_all(path)
}

/// Print a report of everything recorded so far.
///
/// `searched_dirs` is a list of `(label, paths)` groups describing the
/// directories that were searched (config dirs, runtime dirs, ...), rendered
/// with an exists/missing marker.
pub fn print_report(searched_dirs: &[(&str, Vec<PathBuf>)]) {
    let report = REPORT.lock().take().unwrap_or_default();

    println!("\n================ dry-run report ================");

    println!("\n-- searched directories --");
    for (label, dirs) in searched_dirs {
        println!("  {label}:");
        for dir in dirs {
            let marker = if dir.is_dir() { "exists " } else { "missing" };
            println!("    [{marker}] {}", dir.display());
        }
    }

    println!("\n-- config files read (in merge order, lowest to highest priority) --");
    if report.read_configs.is_empty() {
        println!("    (none)");
    }
    for (path, outcome) in &report.read_configs {
        println!("    [{outcome}] {}", path.display());
    }

    println!("\n-- directories that would be created --");
    if report.created_dirs.is_empty() {
        println!("    (none)");
    }
    for dir in &report.created_dirs {
        println!("    {}", dir.display());
    }

    println!("\n-- synthesized editor config (merged result) --");
    println!(
        "  theme: {}",
        report.synthesized_theme.as_deref().unwrap_or("(default)")
    );
    match &report.synthesized_editor {
        Some(toml) => {
            for line in toml.lines() {
                println!("  {line}");
            }
        }
        None => println!("  (default editor config)"),
    }

    println!("\n-- synthesized language config (merged result) --");
    match &report.synthesized_languages {
        Some(summary) => {
            println!("  languages: {}", summary.language_count);
            println!("  grammars:  {}", summary.grammar_count);
            match &summary.grammar_selection {
                Some(sel) => println!("  use-grammars: {sel}"),
                None => println!("  use-grammars: (all grammars selected)"),
            }
        }
        None => println!("  (not loaded)"),
    }

    println!("\n-- actions that would be performed --");
    if report.actions.is_empty() {
        println!("    (none)");
    }
    for (context, detail) in &report.actions {
        println!("    [{context}] {detail}");
    }

    println!("\n=================================================");
}
