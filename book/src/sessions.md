# Sessions

Helix automatically persists the editing session of each project, similar to
VSCode. When you open a project again — by running `hx` without file
arguments in its directory — the previous session is restored.

A session captures:

- The open file-backed buffers.
- The split layout and which document each view shows.
- Every view's selections (cursor positions) and scroll position.
- The focused view.
- The file explorer sidebar state: expanded folders, the selected entry and
  the scroll position.

Sessions are saved continuously (throttled) as you edit, so they survive
crashes, and once more when Helix exits. Quitting with `:quit` does not
discard the session: the buffers that were open before quitting are restored
the next time.

## Storage

Sessions are stored as TOML files, one file per project and session name,
under the session directory (default `~/.cache/helix/sessions`). The file
name is derived from the project root — the nearest ancestor directory
containing `.git`, `.svn`, `.jj` or `.helix` — so every project gets its own
session. The storage directory can be changed with the `--session-dir` flag
or the `HELIX_SESSION_DIR` environment variable.

```sh
# Use a custom session directory
hx --session-dir ~/.local/state/hx-sessions
```

## Named sessions

By default, running `hx` in a project opens its most recently updated
session. Multiple sessions can be kept for the same project by giving them
names, for example one session per feature branch or task:

```sh
# Open (and subsequently save) the session named "bugfix"
hx --session bugfix
```

A named session is stored next to the default one as
`<project-hash>.<name>.toml`. Saving a session updates its modification
time, so an unnamed `hx` always picks up the session you used last.

## Disabling sessions

Sessions can be disabled for a single invocation or permanently via the
environment:

```sh
# One-off
hx --no-session

# Via the environment
HELIX_SESSION=off hx
```

Command line flags take precedence over environment variables:

| Flag | Environment variable | Description |
|---   |---                   |---          |
| `--session [NAME]` | `HELIX_SESSION` | Restore and save a session. Without `NAME`, the most recently updated session of the project is used. `HELIX_SESSION=off` disables sessions entirely. |
| `--no-session` | | Disable session restore and saving. |
| `--session-dir <dir>` | `HELIX_SESSION_DIR` | Directory where session files are stored (default: `~/.cache/helix/sessions`). |

Sessions are only restored when no files are given on the command line:
`hx src/main.rs` opens just that file (the session is still saved on exit
and will then contain that file).
