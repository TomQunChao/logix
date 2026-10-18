<div align="center">

<h1>
<picture>
  <source media="(prefers-color-scheme: dark)" srcset="logo_dark.svg">
  <source media="(prefers-color-scheme: light)" srcset="logo_light.svg">
  <img alt="Helix" height="128" src="logo_light.svg">
</picture>
</h1>

[![Build status](https://github.com/helix-editor/helix/actions/workflows/build.yml/badge.svg)](https://github.com/helix-editor/helix/actions)
[![GitHub Release](https://img.shields.io/github/v/release/helix-editor/helix)](https://github.com/helix-editor/helix/releases/latest)
[![Documentation](https://shields.io/badge/-documentation-452859)](https://docs.helix-editor.com/)
[![GitHub contributors](https://img.shields.io/github/contributors/helix-editor/helix)](https://github.com/helix-editor/helix/graphs/contributors)
[![Matrix Space](https://img.shields.io/matrix/helix-community:matrix.org)](https://matrix.to/#/#helix-community:matrix.org)

</div>

![Screenshot](./screenshot.png)

A [Kakoune](https://github.com/mawww/kakoune) / [Neovim](https://github.com/neovim/neovim) inspired editor, written in Rust.

The editing model is very heavily based on Kakoune; during development I found
myself agreeing with most of Kakoune's design decisions.

For more information, see the [website](https://helix-editor.com) or
[documentation](https://docs.helix-editor.com/).

All shortcuts/keymaps can be found [in the documentation on the website](https://docs.helix-editor.com/keymap.html).

[Troubleshooting](https://github.com/helix-editor/helix/wiki/Troubleshooting)

# Features

- Vim-like modal editing
- Multiple selections
- Built-in language server support
- Smart, incremental syntax highlighting and code editing via tree-sitter

Although it's primarily a terminal-based editor, I am interested in exploring
a custom renderer (similar to Emacs) using wgpu.

Note: Only certain languages have indentation definitions at the moment. Check
`runtime/queries/<lang>/` for `indents.scm`.

# Roadmap

This fork ("logix") aims to grow Helix towards a batteries-included, VS
Code-like professional IDE while keeping the modal editing model.

## Implemented in this fork

- File tree sidebar (`space+e`) with per-type icons, git status colors and
  hidden/ignored toggles. Git status is cached in-process and kept fresh by a
  filesystem watcher, so reopening the tree does not rescan the repository.
- Session save/restore.
- Bufferline shown by default.
- Soft-wrap enabled by default; default theme `base16_transparent`.
- Global search **and replace** across the workspace (`space+/` to search,
  `space+%` to replace).
- Inline git blame (`editor.inline-blame = true`) showing author, relative time
  and abbreviated commit at the end of the cursor line, styled by
  `ui.virtual.git-blame`.
- Code folding, driven by `folds.scm` queries when available and falling back to
  indentation-based folds otherwise. Keybindings live under `z`: `za` toggle,
  `zc` close, `zo` open, `zM` close all, `zR` open all.
- Config dry-run, workspace trust gating.

## Pending / TODO

Other VS Code-like features that are not implemented yet (pick one to build
next):

- **Integrated terminal** (pty-backed).
- **Extension / plugin system** (WASM or scripting runtime).
- **Git source control panel**: stage / commit / push / revert UI, line-level
  blame annotations for every line.
- **Settings UI** and interactive configuration (currently TOML only).
- **LSP / DAP / toolchain install & management UI**.
- **Semantic highlighting** (semantic tokens) — protocol types exist but are not
  wired into the client/renderer.
- **Code Lens**, **call hierarchy**, **type hierarchy**.
- **Peek definition / references** (inline popup).
- **Inline completion / AI assistance** (Copilot-style ghost text).
- Snippet library and interactive snippet editing (only LSP tabstops exist).
- Persistent **Problems panel**.
- **Multi-root workspaces**.
- **Task runner** (`tasks.json`) and **test explorer** integration.
- **Three-way merge / side-by-side diff editor**.
- **Remote development** (SSH / container / WSL).
- **Notebook / Jupyter** support.
- **Live collaboration**.
- Breadcrumbs, minimap, sticky scroll, outline sidebar.
- Theme / icon marketplace and a visual keymap editor.

# Installation

[Installation documentation](https://docs.helix-editor.com/install.html).

[![Packaging status](https://repology.org/badge/vertical-allrepos/helix-editor.svg?exclude_unsupported=1)](https://repology.org/project/helix-editor/versions)

# Contributing

Contributing guidelines can be found [here](./docs/CONTRIBUTING.md).

# Getting help

Your question might already be answered on the [FAQ](https://github.com/helix-editor/helix/wiki/FAQ).

Discuss the project on the community [Matrix Space](https://matrix.to/#/#helix-community:matrix.org) (make sure to join `#helix-editor:matrix.org` if you're on a client that doesn't support Matrix Spaces yet).

# Credits

Thanks to [@jakenvac](https://github.com/jakenvac) for designing the logo!
