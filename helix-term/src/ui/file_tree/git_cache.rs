//! In-process cache for git status, kept fresh by a filesystem watcher.
//!
//! Opening the file tree (`space+e`) runs a `git status` scan. Two things make the full scan
//! expensive on large repositories: enumerating *ignored* files makes gix walk into directories
//! like `target/` (seconds), while the tracked/untracked scan is cheap. So the changed-file scan
//! runs synchronously (fast) while the ignored set is computed on its own thread and merged into
//! the cached result when ready. A [`notify`] watcher keeps the cache fresh afterwards.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{mpsc, Arc, LazyLock, Mutex, MutexGuard, OnceLock},
    thread,
    time::{Duration, Instant},
};

use helix_vcs::{DiffProviderRegistry, FileChange};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tokio::runtime::Handle;

/// How long to wait for the filesystem to settle before recomputing git status. Editor saves and
/// git operations emit bursts of events; waiting avoids recomputing once per event.
const DEBOUNCE: Duration = Duration::from_millis(250);

/// Upper bound on how long events may keep postponing a recompute. Without it, a process that
/// writes files continuously (a build, a log) would starve the refresh forever.
const MAX_DEBOUNCE: Duration = Duration::from_secs(2);

/// Git status for a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitStatus {
    Untracked,
    Added,
    Ignored,
    Modified,
    Deleted,
    Renamed,
    Conflict,
}

impl GitStatus {
    /// The short badge shown after the file name. Empty for statuses that are conveyed by color
    /// alone (e.g. ignored files), so they don't add noise to every entry.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Untracked => "?",
            Self::Added => "A",
            Self::Ignored => "",
            Self::Modified => "M",
            Self::Deleted => "D",
            Self::Renamed => "R",
            Self::Conflict => "C",
        }
    }

    fn from_change(change: &FileChange) -> Self {
        match change {
            FileChange::Untracked { .. } => Self::Untracked,
            FileChange::Added { .. } => Self::Added,
            FileChange::Ignored { .. } => Self::Ignored,
            FileChange::Modified { .. } => Self::Modified,
            FileChange::Conflict { .. } => Self::Conflict,
            FileChange::Deleted { .. } => Self::Deleted,
            FileChange::Renamed { .. } => Self::Renamed,
        }
    }
}

/// The cached status of a single workspace root.
///
/// The two parts are kept apart: `changed` is recomputed on every filesystem change, while
/// `ignored` is expensive to compute and only refreshed when ignore rules or directory structure
/// change.
struct Entry {
    changed: HashMap<PathBuf, GitStatus>,
    ignored: Vec<PathBuf>,
    /// `false` after [`invalidate`] so the next [`load`] recomputes `changed`.
    status_valid: bool,
    /// Whether the (expensive) ignored scan has completed at least once.
    ignored_scanned: bool,
    trust_full: bool,
    /// The runtime this root was first loaded from; the ignored scan is dispatched to it.
    handle: Option<Handle>,
}

#[derive(Default)]
struct Cache {
    entries: HashMap<PathBuf, Entry>,
    /// Roots the watcher watches, including those only watched for `.git` changes.
    watched: HashSet<PathBuf>,
    /// Roots for which no watch could be set up. They are never cached: correctness beats speed
    /// when changes cannot be detected.
    unwatched: HashSet<PathBuf>,
    /// Roots whose ignored scan is currently running on a background thread.
    scanning: HashSet<PathBuf>,
    registry: Option<DiffProviderRegistry>,
}

/// Messages to the watcher worker thread.
enum Msg {
    /// Register `root` with the OS watcher.
    Watch(PathBuf),
    /// Compute the (expensive) ignored set for `root` on a background thread.
    ScanIgnored(PathBuf),
    /// A filesystem event for `path`.
    Event(PathBuf),
}

static CACHE: LazyLock<Arc<Mutex<Cache>>> =
    LazyLock::new(|| Arc::new(Mutex::new(Cache::default())));
/// The command sender to the watcher worker, or `None` if the worker thread could not start.
static WATCHER: OnceLock<Option<mpsc::Sender<Msg>>> = OnceLock::new();

/// Lock the global cache, ignoring poisoning (a panic in the watcher worker must not take the UI
/// down or disable the cache).
fn lock_cache() -> MutexGuard<'static, Cache> {
    CACHE.lock().unwrap_or_else(|err| err.into_inner())
}

/// Return the git status map for `root`, using the cache when possible.
///
/// The first call for a root runs the cheap changed-file scan synchronously and starts watching
/// the repository; the ignored set is computed in the background and merged into later results.
pub fn load(
    root: &Path,
    registry: &DiffProviderRegistry,
    trust_full: bool,
) -> HashMap<PathBuf, GitStatus> {
    let mut unwatched = false;
    {
        let cache = lock_cache();
        if let Some(entry) = cache.entries.get(root) {
            if entry.status_valid {
                return combined(entry);
            }
        } else {
            unwatched = cache.unwatched.contains(root);
        }
    }
    if unwatched {
        // No watcher: never serve a stale cached entry. Only the fast scan is affordable here.
        return compute_changed(root, registry, trust_full);
    }

    let changed = compute_changed(root, registry, trust_full);

    // A watcher is required: without one the cached entry could never be invalidated. If the
    // worker thread could not be started, fall back to scanning on every call.
    let Some(watcher) = watcher_sender() else {
        return changed;
    };

    let need_scan = {
        let mut cache = lock_cache();
        match cache.entries.get_mut(root) {
            Some(entry) => {
                entry.changed = changed;
                entry.status_valid = true;
                !entry.ignored_scanned
            }
            None => {
                cache.registry = Some(registry.clone());
                cache.entries.insert(
                    root.to_path_buf(),
                    Entry {
                        changed,
                        ignored: Vec::new(),
                        status_valid: true,
                        ignored_scanned: false,
                        trust_full,
                        handle: Handle::try_current().ok(),
                    },
                );
                true
            }
        }
    };

    let _ = watcher.send(Msg::Watch(root.to_path_buf()));
    if need_scan {
        let _ = watcher.send(Msg::ScanIgnored(root.to_path_buf()));
    }

    lock_cache()
        .entries
        .get(root)
        .map(combined)
        .unwrap_or_default()
}

/// Drop the cached *changed* status for `root`, forcing the next [`load`] to rescan it. Used after
/// file operations performed in the editor so the tree reflects them immediately instead of
/// waiting for the watcher's debounce. The (expensive) ignored set is kept.
pub fn invalidate(root: &Path) {
    if let Some(entry) = lock_cache().entries.get_mut(root) {
        entry.status_valid = false;
    }
}

/// The changed-file status map plus the ignored entries reported so far.
fn combined(entry: &Entry) -> HashMap<PathBuf, GitStatus> {
    let mut map = entry.changed.clone();
    for path in &entry.ignored {
        map.insert(path.clone(), GitStatus::Ignored);
    }
    map
}

/// The command sender to the watcher worker, starting the thread on first use. `None` if the
/// thread could not be started, in which case the caller must not cache.
fn watcher_sender() -> Option<&'static mpsc::Sender<Msg>> {
    WATCHER
        .get_or_init(|| {
            let (tx, rx) = mpsc::channel();
            let cache = CACHE.clone();
            // Cloned so the worker can feed OS events back through its own command channel.
            let event_tx = tx.clone();
            match thread::Builder::new()
                .name("git-status-watch".to_owned())
                .spawn(move || worker(cache, rx, event_tx))
            {
                Ok(_) => Some(tx),
                Err(err) => {
                    log::warn!("failed to start git status watcher thread: {err}");
                    None
                }
            }
        })
        .as_ref()
}

/// The cheap scan: tracked/untracked/conflict changes, without enumerating ignored files.
fn compute_changed(
    root: &Path,
    registry: &DiffProviderRegistry,
    trust_full: bool,
) -> HashMap<PathBuf, GitStatus> {
    // The provider callback must be `Fn`, so collect through interior mutability.
    let status = RefCell::new(HashMap::new());
    registry.for_each_changed_file_sync(root, trust_full, false, |change| {
        if let Ok(change) = change {
            status
                .borrow_mut()
                .insert(change.path().to_path_buf(), GitStatus::from_change(&change));
        }
        true
    });
    status.into_inner()
}

/// The expensive scan: the paths reported as ignored by git.
fn compute_ignored(root: &Path, registry: &DiffProviderRegistry, trust_full: bool) -> Vec<PathBuf> {
    let ignored = RefCell::new(Vec::new());
    registry.for_each_changed_file_sync(root, trust_full, true, |change| {
        if let Ok(change) = change {
            if matches!(change, FileChange::Ignored { .. }) {
                ignored.borrow_mut().push(change.path().to_path_buf());
            }
        }
        true
    });
    ignored.into_inner()
}

/// Whether `path` is inside a path that the ignored scan reported as ignored.
fn is_ignored(entry: &Entry, path: &Path) -> bool {
    entry
        .ignored
        .iter()
        .any(|ignored| path.starts_with(ignored))
}

/// The watcher worker: owns the OS watcher and processes commands and events.
///
/// The watcher is created here rather than in [`load`] because registering a watch for a large
/// repository can take longer than the UI is allowed to block.
fn worker(cache: Arc<Mutex<Cache>>, rx: mpsc::Receiver<Msg>, event_tx: mpsc::Sender<Msg>) {
    let mut watcher: Option<RecommendedWatcher> = None;

    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Watch(root) => install_watch(&cache, &mut watcher, &event_tx, root),
            Msg::ScanIgnored(root) => start_ignored_scan(&cache, root),
            Msg::Event(first) => {
                let mut dirty: HashSet<PathBuf> = HashSet::new();
                dirty.insert(first);
                let started = Instant::now();

                // Debounce: keep collecting while the filesystem is busy, but never postpone a
                // recompute for longer than `MAX_DEBOUNCE`.
                loop {
                    match rx.recv_timeout(DEBOUNCE) {
                        Ok(Msg::Event(path)) => {
                            dirty.insert(path);
                            if started.elapsed() >= MAX_DEBOUNCE {
                                break;
                            }
                        }
                        Ok(Msg::Watch(root)) => {
                            install_watch(&cache, &mut watcher, &event_tx, root)
                        }
                        Ok(Msg::ScanIgnored(root)) => start_ignored_scan(&cache, root),
                        Err(mpsc::RecvTimeoutError::Timeout) => break,
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                }

                refresh_changed(&cache, &dirty);
                maybe_rescan_ignored(&cache, &dirty);
            }
        }
    }
}

/// Create the OS watcher on first use and watch `root` recursively.
///
/// Recursive watching can fail (e.g. inotify watch limits on huge repositories); in that case we
/// fall back to watching only the git directory, which still catches commits, adds, checkouts
/// and merges. Only if that fails too is caching disabled for the root.
fn install_watch(
    cache: &Arc<Mutex<Cache>>,
    watcher: &mut Option<RecommendedWatcher>,
    event_tx: &mpsc::Sender<Msg>,
    root: PathBuf,
) {
    if watcher.is_none() {
        let tx = event_tx.clone();
        let created = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if let Ok(event) = event {
                for path in event.paths {
                    // The receiver only goes away during shutdown.
                    if tx.send(Msg::Event(path)).is_err() {
                        break;
                    }
                }
            }
        });
        match created {
            Ok(w) => *watcher = Some(w),
            Err(err) => {
                disable(cache, &root, format!("{err}"));
                return;
            }
        }
    }

    if lock_cache().watched.contains(&root) {
        return;
    }
    let Some(watcher) = watcher.as_mut() else {
        return;
    };

    match watcher.watch(&root, RecursiveMode::Recursive) {
        Ok(()) => {
            lock_cache().watched.insert(root);
        }
        Err(err) => {
            let git_dir = root.join(".git");
            if git_dir.exists() && watcher.watch(&git_dir, RecursiveMode::Recursive).is_ok() {
                log::warn!(
                    "watching only {} for git status of {}: {err}",
                    git_dir.display(),
                    root.display()
                );
                lock_cache().watched.insert(root);
            } else {
                disable(cache, &root, format!("{err}"));
            }
        }
    }
}

/// Stop caching `root`: without a watch its status can go stale.
fn disable(cache: &Arc<Mutex<Cache>>, root: &Path, reason: String) {
    log::warn!(
        "git status caching disabled for {}: {reason}",
        root.display()
    );
    let mut cache = cache.lock().unwrap_or_else(|err| err.into_inner());
    cache.unwatched.insert(root.to_path_buf());
    cache.entries.remove(root);
}

/// Kick off the ignored scan for `root` on the tokio blocking pool.
///
/// Running on the blocking pool (rather than a bare thread) lets the scan hand its result back to
/// the UI through the job queue, which needs a runtime context.
fn start_ignored_scan(cache: &Arc<Mutex<Cache>>, root: PathBuf) {
    let (registry, trust_full, handle) = {
        let mut cache = cache.lock().unwrap_or_else(|err| err.into_inner());
        let Some(entry) = cache.entries.get(&root) else {
            return;
        };
        let trust_full = entry.trust_full;
        let handle = entry.handle.clone();
        let Some(registry) = cache.registry.clone() else {
            return;
        };
        if !cache.scanning.insert(root.clone()) {
            // A scan is already running for this root.
            return;
        }
        (registry, trust_full, handle)
    };

    let cache = cache.clone();
    let scan_root = root.clone();
    let dispatch_handle = handle.clone();
    let run = move || {
        let ignored = compute_ignored(&scan_root, &registry, trust_full);
        {
            let mut cache = cache.lock().unwrap_or_else(|err| err.into_inner());
            cache.scanning.remove(&scan_root);
            let Some(entry) = cache.entries.get_mut(&scan_root) else {
                return;
            };
            entry.ignored = ignored;
            entry.ignored_scanned = true;
        }

        // Re-apply the status to an open tree, so ignored entries get dimmed without reopening.
        // `dispatch` needs a runtime context, which the blocking pool provides.
        if let Some(handle) = dispatch_handle {
            handle.spawn(async move {
                crate::job::dispatch(|editor, compositor| {
                    if let Some(editor_view) = compositor.find::<crate::ui::EditorView>() {
                        if let Some(tree) = editor_view.sidebar.as_mut() {
                            tree.reload_git_status(editor);
                        }
                    }
                })
                .await;
            });
        }
    };

    // Without a runtime (e.g. unit tests) the cache is still updated, just without a UI redraw.
    match handle {
        Some(handle) => {
            handle.spawn_blocking(run);
        }
        None => {
            let _ = thread::Builder::new()
                .name("git-status-ignored".to_owned())
                .spawn(run);
        }
    }
}

/// Whether a change to `path` may have changed the ignored set (a new directory or ignore rule).
fn should_rescan_ignored(entry: &Entry, root: &Path, path: &Path) -> bool {
    // Changes inside already-ignored paths cannot change the ignored set.
    if is_ignored(entry, path) {
        return false;
    }
    if path.file_name().is_some_and(|name| name == ".gitignore") {
        return true;
    }
    if path == root.join(".git").join("info").join("exclude") {
        return true;
    }
    // A directory appearing or disappearing at the top level (e.g. `target/`) can introduce a new
    // ignored subtree. `.git` churns constantly, so it is excluded.
    !path.starts_with(root.join(".git")) && path.is_dir() && path.parent() == Some(root)
}

/// Rescan the ignored set for roots whose changed paths might affect it.
fn maybe_rescan_ignored(cache: &Arc<Mutex<Cache>>, paths: &HashSet<PathBuf>) {
    let roots: Vec<PathBuf> = {
        let cache = cache.lock().unwrap_or_else(|err| err.into_inner());
        cache
            .entries
            .iter()
            .filter(|(root, entry)| {
                paths
                    .iter()
                    .any(|path| path.starts_with(root) && should_rescan_ignored(entry, root, path))
            })
            .map(|(root, _)| root.clone())
            .collect()
    };

    for root in roots {
        // Allow `load` to schedule a scan again and start one now.
        if let Some(entry) = cache
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .entries
            .get_mut(&root)
        {
            entry.ignored_scanned = false;
        }
        start_ignored_scan(cache, root);
    }
}

/// Recompute the (cheap) changed-file status for every cached root the changed `paths` touch,
/// ignoring changes that can only be under ignored paths.
fn refresh_changed(cache: &Arc<Mutex<Cache>>, paths: &HashSet<PathBuf>) {
    // Snapshot the targets under the lock, but scan without holding it.
    let targets: Vec<(PathBuf, DiffProviderRegistry, bool)> = {
        let cache = cache.lock().unwrap_or_else(|err| err.into_inner());
        let Some(registry) = cache.registry.clone() else {
            return;
        };
        cache
            .entries
            .iter()
            .filter(|(root, entry)| {
                paths
                    .iter()
                    .any(|path| path.starts_with(root) && !is_ignored(entry, path))
            })
            .map(|(root, entry)| (root.clone(), registry.clone(), entry.trust_full))
            .collect()
    };

    for (root, registry, trust_full) in targets {
        let changed = compute_changed(&root, &registry, trust_full);
        let mut cache = cache.lock().unwrap_or_else(|err| err.into_inner());
        if let Some(entry) = cache.entries.get_mut(&root) {
            entry.changed = changed;
            entry.status_valid = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(changed: HashMap<PathBuf, GitStatus>, ignored: Vec<PathBuf>) -> Entry {
        Entry {
            changed,
            ignored,
            status_valid: true,
            ignored_scanned: true,
            trust_full: true,
            handle: None,
        }
    }

    #[test]
    fn git_status_maps_from_file_change() {
        let path = PathBuf::from("/root/file");
        assert_eq!(
            GitStatus::from_change(&FileChange::Untracked { path: path.clone() }),
            GitStatus::Untracked
        );
        assert_eq!(
            GitStatus::from_change(&FileChange::Added { path: path.clone() }),
            GitStatus::Added
        );
        assert_eq!(
            GitStatus::from_change(&FileChange::Ignored { path: path.clone() }),
            GitStatus::Ignored
        );
        assert_eq!(
            GitStatus::from_change(&FileChange::Modified { path: path.clone() }),
            GitStatus::Modified
        );
        assert_eq!(
            GitStatus::from_change(&FileChange::Deleted { path: path.clone() }),
            GitStatus::Deleted
        );
        assert_eq!(
            GitStatus::from_change(&FileChange::Conflict { path }),
            GitStatus::Conflict
        );
        assert_eq!(
            GitStatus::from_change(&FileChange::Renamed {
                from_path: PathBuf::from("/root/old"),
                to_path: PathBuf::from("/root/new"),
            }),
            GitStatus::Renamed
        );
    }

    #[test]
    fn combined_merges_changed_and_ignored() {
        let entry = entry(
            HashMap::from([(PathBuf::from("/root/a"), GitStatus::Modified)]),
            vec![PathBuf::from("/root/target")],
        );
        let map = combined(&entry);
        assert_eq!(map.get(Path::new("/root/a")), Some(&GitStatus::Modified));
        assert_eq!(
            map.get(Path::new("/root/target")),
            Some(&GitStatus::Ignored)
        );
    }

    #[test]
    fn events_under_ignored_paths_are_filtered_out() {
        let entry = entry(HashMap::new(), vec![PathBuf::from("/root/target")]);
        assert!(is_ignored(
            &entry,
            &PathBuf::from("/root/target/debug/build.o")
        ));
        assert!(!is_ignored(&entry, &PathBuf::from("/root/src/main.rs")));
    }

    #[test]
    fn directory_and_ignore_file_changes_trigger_a_rescan() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join("newdir")).unwrap();
        let entry = entry(HashMap::new(), vec![root.join("target")]);
        assert!(should_rescan_ignored(&entry, root, &root.join("newdir")));
        assert!(should_rescan_ignored(
            &entry,
            root,
            &root.join(".gitignore")
        ));
        assert!(!should_rescan_ignored(
            &entry,
            root,
            &root.join("target/debug/x")
        ));
        // `.git` churn must not trigger rescans.
        assert!(!should_rescan_ignored(
            &entry,
            root,
            &root.join(".git/index")
        ));
        assert!(!should_rescan_ignored(&entry, root, &root.join(".git")));
    }

    #[test]
    fn load_returns_cached_entry_without_scanning() {
        // A root that does not exist: a real scan would find nothing (and never a `Modified`
        // file), so returning the cached status proves the cache path is taken.
        let root = PathBuf::from("/helix/nonexistent/root");
        let changed = HashMap::from([(root.join("file.txt"), GitStatus::Modified)]);
        lock_cache()
            .entries
            .insert(root.clone(), entry(changed, Vec::new()));

        let registry = DiffProviderRegistry::default();
        let loaded = load(&root, &registry, true);
        assert_eq!(
            loaded.get(&root.join("file.txt")),
            Some(&GitStatus::Modified)
        );

        invalidate(&root);
        let cache = lock_cache();
        let entry = cache
            .entries
            .get(&root)
            .expect("entry is kept, only invalidated");
        assert!(!entry.status_valid);
    }
}
