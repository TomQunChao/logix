pub mod config;
pub mod grammar;
pub mod workspace_trust;

use helix_stdx::{env::current_working_dir, path};

use etcetera::base_strategy::{choose_base_strategy, BaseStrategy};
use std::path::{Path, PathBuf};

pub const VERSION_AND_GIT_HASH: &str = env!("VERSION_AND_GIT_HASH");

static RUNTIME_DIRS: once_cell::sync::OnceCell<Vec<PathBuf>> = once_cell::sync::OnceCell::new();

static RUNTIME_DIR_OVERRIDE: once_cell::sync::OnceCell<Option<PathBuf>> =
    once_cell::sync::OnceCell::new();

static CONFIG_DIRS: once_cell::sync::OnceCell<Vec<PathBuf>> = once_cell::sync::OnceCell::new();

static CONFIG_FILE_OVERRIDES: once_cell::sync::OnceCell<Vec<PathBuf>> =
    once_cell::sync::OnceCell::new();

static LANG_CONFIG_FILE_OVERRIDES: once_cell::sync::OnceCell<Vec<PathBuf>> =
    once_cell::sync::OnceCell::new();

static LOG_FILE: once_cell::sync::OnceCell<PathBuf> = once_cell::sync::OnceCell::new();

/// Initialize the config directory chain from the `--config-dir` CLI argument.
///
/// See [`config_dirs`] for the priority order.
pub fn initialize_config_dirs(specified_dir: Option<PathBuf>) {
    CONFIG_DIRS
        .set(prioritize_config_dirs(specified_dir))
        .ok();
}

/// Initialize the config file override chain from the `--config` CLI argument.
///
/// See [`config_file_overrides`] for the priority order.
pub fn initialize_config_file(specified_file: Option<PathBuf>) {
    let overrides = config_file_override_chain(specified_file);
    ensure_parent_dir(&effective_config_file(&overrides));
    CONFIG_FILE_OVERRIDES.set(overrides).ok();
}

/// Initialize the language config file override chain from the `--languages` CLI argument.
///
/// See [`lang_config_file_overrides`] for the priority order.
pub fn initialize_lang_config_files(specified_file: Option<PathBuf>) {
    LANG_CONFIG_FILE_OVERRIDES
        .set(lang_config_file_override_chain(specified_file))
        .ok();
}

/// Store the `--runtime-dir` CLI override.
///
/// The runtime directory list itself is built lazily on first access (see [`runtime_dirs`])
/// so that workspace-relative entries (`.helix/runtime/`) resolve against the final working
/// directory (i.e. after `-w` has been applied).
pub fn set_runtime_dir_override(dir: Option<PathBuf>) {
    RUNTIME_DIR_OVERRIDE.set(dir.map(normalize_path)).ok();
}

pub fn initialize_log_file(specified_file: Option<PathBuf>) {
    let log_file = specified_file.unwrap_or_else(default_log_file);
    ensure_parent_dir(&log_file);
    LOG_FILE.set(log_file).ok();
}

/// Expand `~` and normalize away `.`/`..` components.
fn normalize_path(path: PathBuf) -> PathBuf {
    path::normalize(path::expand_tilde(&path))
}

/// A list of runtime directories from highest to lowest priority
///
/// The priority is:
///
/// 1. the `--runtime-dir` command line argument
/// 2. `HELIX_RUNTIME` (if the environment variable is set)
/// 3. `.helix/runtime/` of the current workspace (only if the workspace is trusted,
///    see [`workspace_trust`])
/// 4. sibling directory to `CARGO_MANIFEST_DIR` (if the environment variable is set)
/// 5. `runtime/` inside each config directory (see [`config_dirs`], highest priority first)
/// 6. `HELIX_DEFAULT_RUNTIME` (if the environment variable is set *at build time*)
/// 7. subdirectory of path to helix executable (always included)
///
/// Postcondition: returns at least two paths (they might not exist).
fn prioritize_runtime_dirs(cli_dir: Option<PathBuf>) -> Vec<PathBuf> {
    const RT_DIR: &str = "runtime";
    // Adding higher priority first
    let mut rt_dirs = Vec::new();
    if let Some(dir) = cli_dir {
        rt_dirs.push(dir);
    }

    if let Ok(dir) = std::env::var("HELIX_RUNTIME") {
        rt_dirs.push(normalize_path(dir.into()));
    }

    if let Some(dir) = trusted_workspace_runtime_dir() {
        log::debug!("trusted workspace runtime dir: {}", dir.to_string_lossy());
        rt_dirs.push(dir);
    }

    if let Ok(dir) = std::env::var("CARGO_MANIFEST_DIR") {
        // this is the directory of the crate being run by cargo, we need the workspace path so we take the parent
        let path = PathBuf::from(dir).parent().unwrap().join(RT_DIR);
        log::debug!("runtime dir: {}", path.to_string_lossy());
        rt_dirs.push(path);
    }

    for dir in config_dirs() {
        rt_dirs.push(dir.join(RT_DIR));
    }

    // If this variable is set during build time, it will always be included
    // in the lookup list. This allows downstream packagers to set a fallback
    // directory to a location that is conventional on their distro so that they
    // need not resort to a wrapper script or a global environment variable.
    if let Some(dir) = std::option_env!("HELIX_DEFAULT_RUNTIME") {
        rt_dirs.push(dir.into());
    }

    // fallback to location of the executable being run
    // canonicalize the path in case the executable is symlinked
    let exe_rt_dir = std::env::current_exe()
        .ok()
        .and_then(|path| std::fs::canonicalize(path).ok())
        .and_then(|path| path.parent().map(|path| path.to_path_buf().join(RT_DIR)))
        .unwrap();
    rt_dirs.push(exe_rt_dir);
    rt_dirs
}

/// `.helix/runtime/` of the current workspace, if it exists and the workspace is trusted.
///
/// Tree-sitter grammars are native libraries, so a workspace-local runtime is only used
/// when the workspace has been *explicitly* trusted (a persisted `:workspace-trust` grant;
/// the grant's hash pin covers everything under `.helix/`). The trust check here
/// deliberately uses the default (conservative) trust configuration: implicit trust levels
/// and trusted globs from the user config are *not* honored, because the runtime directory
/// list may be built before the config file is loaded.
fn trusted_workspace_runtime_dir() -> Option<PathBuf> {
    let dir = workspace_runtime_dir();
    if !dir.is_dir() {
        return None;
    }
    let trust = workspace_trust::WorkspaceTrust::new(workspace_trust::Config::default());
    trust
        .query_current(workspace_trust::TrustQuery::LocalConfig)
        .is_trusted()
        .then_some(dir)
}

/// Runtime directories ordered from highest to lowest priority
///
/// All directories should be checked when looking for files.
///
/// The list is computed lazily on first access, so by then the working directory from `-w`
/// (if any) has been applied and workspace-relative entries resolve correctly.
///
/// Postcondition: returns at least one path (it might not exist).
pub fn runtime_dirs() -> &'static [PathBuf] {
    RUNTIME_DIRS.get_or_init(|| {
        prioritize_runtime_dirs(RUNTIME_DIR_OVERRIDE.get().cloned().flatten())
    })
}

/// Find file with path relative to runtime directory
///
/// `rel_path` should be the relative path from within the `runtime/` directory.
/// The valid runtime directories are searched in priority order and the first
/// file found to exist is returned, otherwise None.
fn find_runtime_file(rel_path: &Path) -> Option<PathBuf> {
    runtime_dirs().iter().find_map(|rt_dir| {
        let path = rt_dir.join(rel_path);
        if path.exists() {
            Some(path)
        } else {
            None
        }
    })
}

/// Find file with path relative to runtime directory
///
/// `rel_path` should be the relative path from within the `runtime/` directory.
/// The valid runtime directories are searched in priority order and the first
/// file found to exist is returned, otherwise the path to the final attempt
/// that failed.
pub fn runtime_file(rel_path: impl AsRef<Path>) -> PathBuf {
    find_runtime_file(rel_path.as_ref()).unwrap_or_else(|| {
        runtime_dirs()
            .last()
            .map(|dir| dir.join(rel_path))
            .unwrap_or_default()
    })
}

/// The system default config directory (e.g. `$XDG_CONFIG_HOME/helix` on Linux).
fn system_config_dir() -> PathBuf {
    let strategy = choose_base_strategy().expect("Unable to find the config directory!");
    let mut path = strategy.config_dir();
    path.push("helix");
    path
}

fn prioritize_config_dirs(specified_dir: Option<PathBuf>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dir) = specified_dir {
        dirs.push(normalize_path(dir));
    }
    if let Ok(dir) = std::env::var("HELIX_CONFIG_DIR") {
        dirs.push(normalize_path(dir.into()));
    }
    dirs.push(system_config_dir());
    dirs
}

/// Config directories ordered from highest to lowest priority:
///
/// 1. the `--config-dir` command line argument
/// 2. the `HELIX_CONFIG_DIR` environment variable
/// 3. the system default (e.g. `$XDG_CONFIG_HOME/helix` on Linux)
///
/// Config files (`config.toml`, `languages.toml`, `themes/`, `runtime/`) missing from a
/// higher-priority directory fall back to lower-priority ones. The system default is always
/// present, so the list is never empty.
pub fn config_dirs() -> &'static [PathBuf] {
    CONFIG_DIRS.get_or_init(|| prioritize_config_dirs(None))
}

/// The highest-priority config directory. Files created by helix (e.g. via `:config-open`)
/// are placed here.
pub fn config_dir() -> PathBuf {
    config_dirs()
        .first()
        .expect("config dirs are never empty")
        .clone()
}

pub fn cache_dir() -> PathBuf {
    // TODO: allow env var override
    let strategy = choose_base_strategy().expect("Unable to find the cache directory!");
    let mut path = strategy.cache_dir();
    path.push("helix");
    path
}

pub fn data_dir() -> PathBuf {
    let strategy = choose_base_strategy().expect("Unable to find the data directory!");
    let mut path = strategy.data_dir();
    path.push("helix");
    path
}

/// The highest-priority config file: the last entry of [`config_file_overrides`], or
/// `config.toml` inside the highest-priority config directory.
pub fn config_file() -> PathBuf {
    effective_config_file(config_file_overrides())
}

pub fn log_file() -> PathBuf {
    LOG_FILE.get().map(|path| path.to_path_buf()).unwrap()
}

pub fn workspace_config_file() -> PathBuf {
    find_workspace().0.join(".helix").join("config.toml")
}

pub fn workspace_lang_config_file() -> PathBuf {
    find_workspace().0.join(".helix").join("languages.toml")
}

pub fn workspace_runtime_dir() -> PathBuf {
    find_workspace().0.join(".helix").join("runtime")
}

fn config_file_override_chain(specified_file: Option<PathBuf>) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(file) = std::env::var("HELIX_CONFIG_FILE") {
        files.push(normalize_path(file.into()));
    }
    if let Some(file) = specified_file {
        files.push(normalize_path(file));
    }
    files
}

fn lang_config_file_override_chain(specified_file: Option<PathBuf>) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(file) = std::env::var("HELIX_LANGUAGES_FILE") {
        files.push(normalize_path(file.into()));
    }
    if let Some(file) = specified_file {
        files.push(normalize_path(file));
    }
    files
}

fn effective_config_file(overrides: &[PathBuf]) -> PathBuf {
    overrides.last().cloned().unwrap_or_else(default_config_file)
}

/// Explicitly specified config files ordered from lowest to highest priority:
///
/// 1. the `HELIX_CONFIG_FILE` environment variable
/// 2. the `--config` command line argument
///
/// These are merged *on top of* every `config.toml` found in [`config_dirs`] and on top of
/// the workspace's `.helix/config.toml`, so an explicitly specified key always wins.
pub fn config_file_overrides() -> &'static [PathBuf] {
    CONFIG_FILE_OVERRIDES.get_or_init(|| config_file_override_chain(None))
}

/// Explicitly specified language config files ordered from lowest to highest priority:
/// `HELIX_LANGUAGES_FILE`, then `--languages`.
///
/// These are merged *on top of* every `languages.toml` found in [`config_dirs`] and on top
/// of the workspace's `.helix/languages.toml`.
pub fn lang_config_file_overrides() -> &'static [PathBuf] {
    LANG_CONFIG_FILE_OVERRIDES.get_or_init(|| lang_config_file_override_chain(None))
}

/// The highest-priority language config file: the last entry of
/// [`lang_config_file_overrides`], or `languages.toml` inside the highest-priority config
/// directory.
pub fn lang_config_file() -> PathBuf {
    lang_config_file_overrides()
        .last()
        .cloned()
        .unwrap_or_else(|| config_dir().join("languages.toml"))
}

pub fn default_log_file() -> PathBuf {
    cache_dir().join("helix.log")
}

/// Merge two TOML documents, merging values from `right` onto `left`
///
/// `merge_depth` sets the nesting depth up to which values are merged instead
/// of overridden.
///
/// When a table exists in both `left` and `right`, the merged table consists of
/// all keys in `left`'s table unioned with all keys in `right` with the values
/// of `right` being merged recursively onto values of `left`.
///
/// `crate::merge_toml_values(a, b, 3)` combines, for example:
///
/// b:
/// ```toml
/// [[language]]
/// name = "toml"
/// language-server = { command = "taplo", args = ["lsp", "stdio"] }
/// ```
/// a:
/// ```toml
/// [[language]]
/// language-server = { command = "/usr/bin/taplo" }
/// ```
///
/// into:
/// ```toml
/// [[language]]
/// name = "toml"
/// language-server = { command = "/usr/bin/taplo" }
/// ```
///
/// thus it overrides the third depth-level of b with values of a if they exist,
/// but otherwise merges their values
pub fn merge_toml_values(left: toml::Value, right: toml::Value, merge_depth: usize) -> toml::Value {
    use toml::Value;

    fn get_name(v: &Value) -> Option<&str> {
        v.get("name").and_then(Value::as_str)
    }

    match (left, right) {
        (Value::Array(mut left_items), Value::Array(right_items)) => {
            if merge_depth > 0 {
                left_items.reserve(right_items.len());
                for rvalue in right_items {
                    let lvalue = get_name(&rvalue)
                        .and_then(|rname| {
                            left_items.iter().position(|v| get_name(v) == Some(rname))
                        })
                        .map(|lpos| left_items.remove(lpos));
                    let mvalue = match lvalue {
                        Some(lvalue) => merge_toml_values(lvalue, rvalue, merge_depth - 1),
                        None => rvalue,
                    };
                    left_items.push(mvalue);
                }
                Value::Array(left_items)
            } else {
                Value::Array(right_items)
            }
        }
        (Value::Table(mut left_map), Value::Table(right_map)) => {
            if merge_depth > 0 {
                for (rname, rvalue) in right_map {
                    match left_map.remove(&rname) {
                        Some(lvalue) => {
                            let merged_value = merge_toml_values(lvalue, rvalue, merge_depth - 1);
                            left_map.insert(rname, merged_value);
                        }
                        None => {
                            left_map.insert(rname, rvalue);
                        }
                    }
                }
                Value::Table(left_map)
            } else {
                Value::Table(right_map)
            }
        }
        // Catch everything else we didn't handle, and use the right value
        (_, value) => value,
    }
}

/// Finds the current workspace folder.
/// Used as a ceiling dir for LSP root resolution, the filepicker and potentially as a future filewatching root
///
/// This function starts searching the FS upward from the CWD
/// and returns the first directory that contains either `.git`, `.svn`, `.jj` or `.helix`.
/// If no workspace was found returns (CWD, true).
/// Otherwise (workspace, false) is returned
pub fn find_workspace() -> (PathBuf, bool) {
    let current_dir = current_working_dir();
    find_workspace_in(current_dir)
}

pub fn find_workspace_in(dir: impl AsRef<Path>) -> (PathBuf, bool) {
    let dir = dir.as_ref();
    for ancestor in dir.ancestors() {
        if ancestor.join(".git").exists()
            || ancestor.join(".svn").exists()
            || ancestor.join(".jj").exists()
            || ancestor.join(".helix").exists()
        {
            return (ancestor.to_owned(), false);
        }
    }

    (dir.to_owned(), true)
}

fn default_config_file() -> PathBuf {
    config_dir().join("config.toml")
}

fn ensure_parent_dir(path: &Path) {
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent).ok();
        }
    }
}

#[cfg(test)]
mod merge_toml_tests {
    use std::str;

    use super::merge_toml_values;
    use toml::Value;

    #[test]
    fn language_toml_map_merges() {
        const USER: &str = r#"
        [[language]]
        name = "nix"
        test = "bbb"
        indent = { tab-width = 4, unit = "    ", test = "aaa" }
        "#;

        let base = include_bytes!("../../languages.toml");
        let base = str::from_utf8(base).expect("Couldn't parse built-in languages config");
        let base: Value = toml::from_str(base).expect("Couldn't parse built-in languages config");
        let user: Value = toml::from_str(USER).unwrap();

        let merged = merge_toml_values(base, user, 3);
        let languages = merged.get("language").unwrap().as_array().unwrap();
        let nix = languages
            .iter()
            .find(|v| v.get("name").unwrap().as_str().unwrap() == "nix")
            .unwrap();
        let nix_indent = nix.get("indent").unwrap();

        // We changed tab-width and unit in indent so check them if they are the new values
        assert_eq!(
            nix_indent.get("tab-width").unwrap().as_integer().unwrap(),
            4
        );
        assert_eq!(nix_indent.get("unit").unwrap().as_str().unwrap(), "    ");
        // We added a new keys, so check them
        assert_eq!(nix.get("test").unwrap().as_str().unwrap(), "bbb");
        assert_eq!(nix_indent.get("test").unwrap().as_str().unwrap(), "aaa");
        // We didn't change comment-token so it should be same
        assert_eq!(nix.get("comment-token").unwrap().as_str().unwrap(), "#");
    }

    #[test]
    fn language_toml_nested_array_merges() {
        const USER: &str = r#"
        [[language]]
        name = "typescript"
        language-server = { command = "deno", args = ["lsp"] }
        "#;

        let base = include_bytes!("../../languages.toml");
        let base = str::from_utf8(base).expect("Couldn't parse built-in languages config");
        let base: Value = toml::from_str(base).expect("Couldn't parse built-in languages config");
        let user: Value = toml::from_str(USER).unwrap();

        let merged = merge_toml_values(base, user, 3);
        let languages = merged.get("language").unwrap().as_array().unwrap();
        let ts = languages
            .iter()
            .find(|v| v.get("name").unwrap().as_str().unwrap() == "typescript")
            .unwrap();
        assert_eq!(
            ts.get("language-server")
                .unwrap()
                .get("args")
                .unwrap()
                .as_array()
                .unwrap(),
            &vec![Value::String("lsp".into())]
        )
    }
}
