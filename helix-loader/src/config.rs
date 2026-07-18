use std::str::from_utf8;

use crate::workspace_trust::{TrustQuery, WorkspaceTrust};

/// Default built-in languages.toml.
pub fn default_lang_config() -> toml::Value {
    let default_config = include_bytes!("../../languages.toml");
    toml::from_str(from_utf8(default_config).unwrap())
        .expect("Could not parse built-in languages.toml to valid toml")
}

/// User configured languages.toml file, merged with the default config.
///
/// Layers are merged from lowest to highest priority (later layers win):
///
/// 1. `languages.toml` inside each config directory (system default, `HELIX_CONFIG_DIR`,
///    `--config-dir`; see [`crate::config_dirs`])
/// 2. workspace-local `.helix/languages.toml`, merged in only when the current workspace is
///    trusted for [`TrustQuery::LocalConfig`]
/// 3. explicitly specified files (`HELIX_LANGUAGES_FILE`, then `--languages`;
///    see [`crate::lang_config_file_overrides`])
pub fn user_lang_config(trust: &WorkspaceTrust) -> Result<toml::Value, toml::de::Error> {
    let mut files: Vec<std::path::PathBuf> = crate::config_dirs()
        .iter()
        .rev()
        .map(|dir| dir.join("languages.toml"))
        .collect();

    if trust.query_current(TrustQuery::LocalConfig).is_trusted() {
        files.push(crate::workspace_lang_config_file());
    }

    files.extend(crate::lang_config_file_overrides().iter().cloned());

    let config = files
        .iter()
        .filter_map(|file| {
            std::fs::read_to_string(file)
                .map(|config| toml::from_str(&config))
                .ok()
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .fold(default_lang_config(), |a, b| {
            crate::merge_toml_values(a, b, 3)
        });

    Ok(config)
}
