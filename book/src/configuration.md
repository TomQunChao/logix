# Configuration

To override global configuration parameters, create a `config.toml` file located in your config directory:

- Linux and Mac: `~/.config/helix/config.toml`
- Windows: `%AppData%\helix\config.toml`

> 💡 You can easily open the config file by typing `:config-open` within Helix normal mode.

Example config:

```toml
theme = "onedark"

[editor]
line-number = "relative"
mouse = false

[editor.cursor-shape]
insert = "bar"
normal = "block"
select = "underline"

[editor.file-picker]
hidden = false
```

You can use a custom configuration file by specifying it with the `-c` or
`--config` command line argument, for example `hx -c path/to/custom-config.toml`.
You can reload the config file by issuing the `:config-reload` command. Alternatively, on Unix operating systems, you can reload it by sending the USR1
signal to the Helix process, such as by using the command `pkill -USR1 hx`.

Finally, you can have a `config.toml` and a `languages.toml` local to a project by putting it under a `.helix` directory in your repository.
Its settings will be merged with the configuration directory and the built-in configuration.

## Configuration directories

Helix looks for `config.toml`, `languages.toml`, `themes/` and `runtime/` in a
chain of config directories. The chain is ordered from highest to lowest
priority:

1. The `--config-dir <dir>` command line argument.
2. The `HELIX_CONFIG_DIR` environment variable.
3. The system default config directory (`~/.config/helix` on Linux and macOS,
   `%AppData%\helix` on Windows).

Files missing from a higher-priority directory fall back to lower-priority
ones, and all directories that contain the file are merged together (see
below). Files created by Helix (e.g. via `:config-open`) are placed in the
highest-priority directory.

## Configuration file merging

`config.toml` and `languages.toml` are not loaded from a single location —
all applicable layers are deep-merged, with higher layers overriding lower
ones on conflicting keys. From lowest to highest priority:

1. The built-in defaults (`languages.toml` is compiled into the binary;
   `config.toml` uses built-in default values).
2. `config.toml` / `languages.toml` in each config directory (system default
   first, then `HELIX_CONFIG_DIR`, then `--config-dir`).
3. The workspace's `.helix/config.toml` / `.helix/languages.toml`, but only
   when the workspace is [trusted](./workspace-trust.md).
4. An explicitly specified file: the `HELIX_CONFIG_FILE` /
   `HELIX_LANGUAGES_FILE` environment variable.
5. An explicitly specified file: the `-c`/`--config` or `--languages` command
   line argument (highest priority).

An explicitly specified file that does not exist is a startup error, while
missing files in config directories are skipped silently.

## Verifying the configuration with `--dry-run`

`hx --dry-run` exercises the whole configuration pipeline without modifying
the system: no directories are created, no files are written, and no editor
is started. It prints a report containing:

- the config and runtime directories that were searched (with an
  exists/missing marker each),
- every `config.toml` / `languages.toml` layer that was probed, in merge
  order, with its outcome (loaded / not found / error),
- the directories that *would* be created (e.g. a `--config-dir` that does
  not exist yet, the log file's parent directory),
- the synthesized (merged) editor configuration and theme, plus a summary of
  the merged language configuration (language/grammar counts and the
  `use-grammars` selection).

This is useful to check that the config directory chain, file layering and
workspace-trust gates behave as expected, for example when bootstrapping a
new config directory:

```sh
hx --dry-run --config-dir ./my-config --config ./extra.toml
```

Combined with `--grammar fetch` or `--grammar build`, the dry-run additionally
reports which grammars would be fetched or compiled. The `git` invocations
and compiler runs are reported but **not** executed, so no network access
takes place and nothing is written:

```sh
hx --dry-run --grammar fetch
hx --dry-run --config-dir ./my-config --grammar build
```

## Environment variables

| Variable | Description |
|---       |---          |
| `HELIX_CONFIG_DIR` | Config directory (overridden by `--config-dir`). |
| `HELIX_CONFIG_FILE` | Config file (overridden by `--config`). |
| `HELIX_LANGUAGES_FILE` | Language config file (overridden by `--languages`). |
| `HELIX_RUNTIME` | Runtime directory (overridden by `--runtime-dir`). See [multiple runtime directories](./building-from-source.md#multiple-runtime-directories). |
| `HELIX_LOG_LEVEL` | Log level used for integration logging. |
| `HELIX_SESSION` | Session behavior (overridden by `--session`/`--no-session`). Set to `off` to disable sessions, or to a name to select a named session. See [sessions](./sessions.md). |
| `HELIX_SESSION_DIR` | Session storage directory (overridden by `--session-dir`). See [sessions](./sessions.md). |

