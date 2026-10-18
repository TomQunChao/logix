//! In-process cache for git status, kept fresh by a filesystem watcher.
//!
//! Opening the file tree (`space+e`) runs a full `git status` scan, which is expensive on large
//! repositories. The result is cached per workspace root and a [`notify`] watcher recomputes it
//! in the background whenever the repository changes, so opening the tree is a cheap cache hit
//! instead of a scan.
//!
//! Both the scan and the watcher setup happen outside the UI's critical path where possible: the
//! watcher runs on its own thread and is only *asked* to watch a root, so opening the tree never
//! blocks on registering filesystem watches.

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
struct Entry {
    status: HashMap<PathBuf, GitStatus>,
    trust_full: bool,
    /// Ignored files/directories reported by the last scan. Events under these paths cannot
    /// change the status, so they are filtered out to keep e.g. build output from triggering
    /// scans (the watched tree includes ignored directories).
    ignored: Vec<PathBuf>,
}

#[derive(Default)]
struct Cache {
    entries: HashMap<PathBuf, Entry>,
    /// Roots the watcher watches, including those only watched for `.git` changes.
    watched: HashSet<PathBuf>,
    /// Roots for which no watch could be set up. They are never cached: correctness beats speed
    /// when changes cannot be detected.
    unwatched: HashSet<PathBuf>,
    registry: Option<DiffProviderRegistry>,
}

/// Messages to the watcher worker thread.
enum Msg {
    /// Register `root` with the OS watcher.
    Watch(PathBuf),
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
/// The first call for a root runs the scan synchronously (the unavoidable cost of the first
/// open) and asks the watcher thread to watch the repository. Afterwards the watcher keeps the
/// entry fresh in the background.
pub fn load(
    root: &Path,
    registry: &DiffProviderRegistry,
    trust_full: bool,
) -> HashMap<PathBuf, GitStatus> {
    {
        let cache = lock_cache();
        if let Some(entry) = cache.entries.get(root) {
            return entry.status.clone();
        }
        if cache.unwatched.contains(root) {
            drop(cache);
            return compute(root, registry, trust_full);
        }
    }

    // Scan without holding the lock; a scan can take a while on large repositories.
    let status = compute(root, registry, trust_full);

    // A watcher is required: without one the cached entry could never be invalidated. If the
    // worker thread could not be started, fall back to scanning on every call.
    let Some(watcher) = watcher_sender() else {
        return status;
    };

    {
        let mut cache = lock_cache();
        // Another thread may have populated the entry while we scanned.
        if let Some(entry) = cache.entries.get(root) {
            return entry.status.clone();
        }
        cache.registry = Some(registry.clone());
        cache.entries.insert(
            root.to_path_buf(),
            Entry {
                ignored: ignored_roots(&status),
                status: status.clone(),
                trust_full,
            },
        );
    }

    // Ask the watcher thread to watch the root. This is non-blocking; if registration fails the
    // worker drops the entry and disables caching for the root.
    let _ = watcher.send(Msg::Watch(root.to_path_buf()));
    status
}

/// Drop the cached status for `root`, forcing the next [`load`] to rescan. Used after file
/// operations performed in the editor so the tree reflects them immediately instead of waiting
/// for the watcher's debounce. The watch itself is kept.
pub fn invalidate(root: &Path) {
    lock_cache().entries.remove(root);
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

/// Run a full `git status`-style scan for `root`, including ignored files.
fn compute(
    root: &Path,
    registry: &DiffProviderRegistry,
    trust_full: bool,
) -> HashMap<PathBuf, GitStatus> {
    // The provider callback must be `Fn`, so collect through interior mutability.
    let status = RefCell::new(HashMap::new());
    registry.for_each_changed_file_sync(root, trust_full, true, |change| {
        if let Ok(change) = change {
            status
                .borrow_mut()
                .insert(change.path().to_path_buf(), GitStatus::from_change(&change));
        }
        true
    });
    status.into_inner()
}

/// The ignored paths reported by a scan.
fn ignored_roots(status: &HashMap<PathBuf, GitStatus>) -> Vec<PathBuf> {
    status
        .iter()
        .filter(|(_, status)| **status == GitStatus::Ignored)
        .map(|(path, _)| path.clone())
        .collect()
}

/// Whether `path` is inside a path that the last scan reported as ignored.
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
                        Err(mpsc::RecvTimeoutError::Timeout) => break,
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                }

                refresh(&cache, &dirty);
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

/// Recompute every cached root that the changed `paths` touch, ignoring changes that can only be
/// under ignored paths.
fn refresh(cache: &Arc<Mutex<Cache>>, paths: &HashSet<PathBuf>) {
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
        let status = compute(&root, &registry, trust_full);
        let mut cache = cache.lock().unwrap_or_else(|err| err.into_inner());
        if let Some(entry) = cache.entries.get_mut(&root) {
            entry.ignored = ignored_roots(&status);
            entry.status = status;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn ignored_roots_lists_only_ignored_paths() {
        let mut status = HashMap::new();
        status.insert(PathBuf::from("/root/target"), GitStatus::Ignored);
        status.insert(PathBuf::from("/root/src/main.rs"), GitStatus::Modified);

        let ignored = ignored_roots(&status);
        assert_eq!(ignored, vec![PathBuf::from("/root/target")]);
    }

    #[test]
    fn events_under_ignored_paths_are_filtered_out() {
        let entry = Entry {
            status: HashMap::new(),
            trust_full: true,
            ignored: vec![PathBuf::from("/root/target")],
        };
        assert!(is_ignored(
            &entry,
            &PathBuf::from("/root/target/debug/build.o")
        ));
        assert!(!is_ignored(&entry, &PathBuf::from("/root/src/main.rs")));
    }

    #[test]
    fn load_returns_cached_entry_without_scanning() {
        // A root that does not exist: a real scan would find nothing (and never a `Modified`
        // file), so returning the cached status proves the cache path is taken.
        let root = PathBuf::from("/helix/nonexistent/root");
        let mut status = HashMap::new();
        status.insert(root.join("file.txt"), GitStatus::Modified);
        lock_cache().entries.insert(
            root.clone(),
            Entry {
                status,
                trust_full: true,
                ignored: Vec::new(),
            },
        );

        let registry = DiffProviderRegistry::default();
        let loaded = load(&root, &registry, true);
        assert_eq!(
            loaded.get(&root.join("file.txt")),
            Some(&GitStatus::Modified)
        );

        invalidate(&root);
        assert!(lock_cache().entries.get(&root).is_none());
    }
}
