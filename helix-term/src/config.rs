use crate::keymap;
use crate::keymap::{merge_keys, KeyTrie};
use helix_loader::merge_toml_values;
use helix_view::{document::Mode, theme};
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt::Display;
use std::fs;
use std::io::Error as IOError;
use toml::de::Error as TomlError;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub theme: Option<theme::Config>,
    pub keys: HashMap<Mode, KeyTrie>,
    pub editor: helix_view::editor::Config,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigRaw {
    pub theme: Option<theme::Config>,
    pub keys: Option<HashMap<Mode, KeyTrie>>,
    pub editor: Option<toml::Value>,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            theme: None,
            keys: keymap::default(),
            editor: helix_view::editor::Config::default(),
        }
    }
}

#[derive(Debug)]
pub enum ConfigLoadError {
    BadConfig(TomlError),
    Error(IOError),
}

impl Default for ConfigLoadError {
    fn default() -> Self {
        ConfigLoadError::Error(IOError::new(std::io::ErrorKind::NotFound, "place holder"))
    }
}

impl Display for ConfigLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigLoadError::BadConfig(err) => err.fmt(f),
            ConfigLoadError::Error(err) => err.fmt(f),
        }
    }
}

impl Config {
    /// Merge config layers from lowest to highest priority.
    ///
    /// * keys are merged on top of the default keymap in layer order
    /// * `editor` tables are deep-merged (see [`merge_toml_values`])
    /// * `theme` is taken from the highest-priority layer that sets it
    fn merge_layers(layers: Vec<ConfigRaw>) -> Result<Config, ConfigLoadError> {
        let mut keys = keymap::default();
        let mut theme = None;
        let mut editor: Option<toml::Value> = None;

        for layer in layers {
            if let Some(layer_keys) = layer.keys {
                merge_keys(&mut keys, layer_keys);
            }
            if layer.theme.is_some() {
                theme = layer.theme;
            }
            if let Some(layer_editor) = layer.editor {
                editor = Some(match editor {
                    Some(merged) => merge_toml_values(merged, layer_editor, 3),
                    None => layer_editor,
                });
            }
        }

        let editor = match editor {
            Some(value) => value.try_into().map_err(ConfigLoadError::BadConfig)?,
            None => helix_view::editor::Config::default(),
        };

        Ok(Config { theme, keys, editor })
    }

    /// Read and parse a single config layer. Missing files are skipped (`Ok(None)`); files
    /// that exist but cannot be read or parsed are errors.
    fn load_layer(
        path: &std::path::Path,
        explicit: bool,
    ) -> Result<Option<ConfigRaw>, ConfigLoadError> {
        match fs::read_to_string(path) {
            Ok(file) => toml::from_str(&file)
                .map(Some)
                .map_err(ConfigLoadError::BadConfig),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                if explicit {
                    log::warn!("config file {} not found", path.display());
                }
                Ok(None)
            }
            Err(err) => Err(ConfigLoadError::Error(err)),
        }
    }

    /// Load the configuration, merging layers from lowest to highest priority:
    ///
    /// 1. `config.toml` inside each config directory (system default, `HELIX_CONFIG_DIR`,
    ///    `--config-dir`; see [`helix_loader::config_dirs`])
    /// 2. the workspace's `.helix/config.toml`, if the workspace is trusted
    /// 3. explicit files: `HELIX_CONFIG_FILE`, then `--config`
    ///    (see [`helix_loader::config_file_overrides`])
    pub fn load_default() -> Result<Config, ConfigLoadError> {
        let mut dir_layers = Vec::new();
        for dir in helix_loader::config_dirs().iter().rev() {
            if let Some(layer) = Self::load_layer(&dir.join("config.toml"), false)? {
                dir_layers.push(layer);
            }
        }

        let mut override_layers = Vec::new();
        for file in helix_loader::config_file_overrides() {
            if let Some(layer) = Self::load_layer(file, true)? {
                override_layers.push(layer);
            }
        }

        // The workspace-trust gate is driven by the user-level (non-workspace) layers only.
        let user_config = Self::merge_layers(
            dir_layers
                .iter()
                .cloned()
                .chain(override_layers.iter().cloned())
                .collect(),
        )?;

        let trust = helix_loader::workspace_trust::WorkspaceTrust::new(
            (&user_config.editor.workspace_trust).into(),
        );

        let mut merged = user_config.clone();
        if trust
            .query_current(helix_loader::workspace_trust::TrustQuery::LocalConfig)
            .is_trusted()
        {
            if let Some(workspace_layer) =
                Self::load_layer(&helix_loader::workspace_config_file(), false)?
            {
                // The workspace layer sits above the config directories but below the
                // explicit file overrides.
                let mut layers = dir_layers;
                layers.push(workspace_layer);
                layers.extend(override_layers);
                merged = Self::merge_layers(layers)?;
            }
        }

        // editor.workspace-trust is global/user-scope only. Without this override, a
        // workspace's `.helix/config.toml` could set `level = "insecure"`; once the user
        // trusted *that* workspace, refresh_config would re-load with the override merged in
        // and from then on every subsequent workspace in the session would be implicitly
        // trusted. Pin the gate's own configuration to the user-level layers.
        merged.editor.workspace_trust = user_config.editor.workspace_trust;
        Ok(merged)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Config {
        fn load_test(config: &str) -> Config {
            Config::merge_layers(vec![toml::from_str(config).unwrap()]).unwrap()
        }
    }

    #[test]
    fn parsing_keymaps_config_file() {
        use crate::keymap;
        use helix_core::hashmap;
        use helix_view::document::Mode;

        let sample_keymaps = r#"
            [keys.insert]
            y = "move_line_down"
            S-C-a = "delete_selection"

            [keys.normal]
            A-F12 = "move_next_word_end"
        "#;

        let mut keys = keymap::default();
        merge_keys(
            &mut keys,
            hashmap! {
                Mode::Insert => keymap!({ "Insert mode"
                    "y" => move_line_down,
                    "S-C-a" => delete_selection,
                }),
                Mode::Normal => keymap!({ "Normal mode"
                    "A-F12" => move_next_word_end,
                }),
            },
        );

        assert_eq!(
            Config::load_test(sample_keymaps),
            Config {
                keys,
                ..Default::default()
            }
        );
    }

    #[test]
    fn keys_resolve_to_correct_defaults() {
        // From serde default
        let default_keys = Config::load_test("").keys;
        assert_eq!(default_keys, keymap::default());

        // From the Default trait
        let default_keys = Config::default().keys;
        assert_eq!(default_keys, keymap::default());
    }

    #[test]
    fn merge_layers_editor_deep_merges_in_order() {
        // Layers are given lowest to highest priority: the highest layer wins on
        // conflicting keys, but keys only present in lower layers are preserved.
        let low: crate::config::ConfigRaw = toml::from_str(
            r#"
            [editor]
            scrolloff = 3
            mouse = false
            "#,
        )
        .unwrap();
        let high: crate::config::ConfigRaw = toml::from_str(
            r#"
            [editor]
            scrolloff = 7
            "#,
        )
        .unwrap();

        let merged = Config::merge_layers(vec![low, high]).unwrap();
        assert_eq!(merged.editor.scrolloff, 7);
        assert!(!merged.editor.mouse);
    }

    #[test]
    fn merge_layers_theme_taken_from_highest_layer_that_sets_it() {
        use helix_view::theme;

        let low: crate::config::ConfigRaw = toml::from_str(r#"theme = "low""#).unwrap();
        let mid: crate::config::ConfigRaw = toml::from_str("").unwrap();
        let high: crate::config::ConfigRaw = toml::from_str(r#"theme = "high""#).unwrap();

        let merged = Config::merge_layers(vec![low, mid, high]).unwrap();
        assert_eq!(merged.theme, Some(theme::Config::Constant("high".into())),);

        // A layer without a theme does not reset a lower layer's choice.
        let low: crate::config::ConfigRaw = toml::from_str(r#"theme = "low""#).unwrap();
        let high: crate::config::ConfigRaw = toml::from_str("").unwrap();
        let merged = Config::merge_layers(vec![low, high]).unwrap();
        assert_eq!(merged.theme, Some(theme::Config::Constant("low".into())),);
    }

    #[test]
    fn merge_layers_keys_merge_in_order() {
        use helix_core::hashmap;
        use helix_view::document::Mode;

        let low: crate::config::ConfigRaw = toml::from_str(
            r#"
            [keys.normal]
            a = "move_line_down"
            b = "move_line_up"
            "#,
        )
        .unwrap();
        let high: crate::config::ConfigRaw = toml::from_str(
            r#"
            [keys.normal]
            b = "delete_selection"
            "#,
        )
        .unwrap();

        let merged = Config::merge_layers(vec![low, high]).unwrap();

        let mut expected = keymap::default();
        merge_keys(
            &mut expected,
            hashmap! {
                Mode::Normal => keymap!({ "Normal mode"
                    "a" => move_line_down,
                    "b" => move_line_up,
                }),
            },
        );
        merge_keys(
            &mut expected,
            hashmap! {
                Mode::Normal => keymap!({ "Normal mode"
                    "b" => delete_selection,
                }),
            },
        );

        assert_eq!(merged.keys, expected);
    }
}
