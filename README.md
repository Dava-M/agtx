# agtx

`agtx` is a lightweight, read-only terminal observer for agent-driven changes.
It shows the repository tree with git status colors, the uncommitted diff of the
selected file, and refreshes itself as files change on disk.

It never writes to the repository: git is only invoked for read operations
(`status`, `ls-files`, `diff`, `rev-parse`), and files are never modified.

## Build

```sh
cargo build --release
```

The binary is `target/release/agtx`.

## Run

```sh
./target/release/agtx [path]
```

`path` defaults to `.`; the repository root is resolved with `git rev-parse`.
The binary can also run from `cargo run --release -- [path]`.

## Configuration

Config path, in order of precedence:

1. `--config <path>` (must exist; a missing explicit path is an error),
2. `$XDG_CONFIG_HOME/agtx/config.toml`,
3. `$HOME/.config/agtx/config.toml`.

When the default file is absent, the built-in defaults below are used.

Full default TOML:

```toml
refresh_debounce_ms = 200
max_diff_bytes = 2000000
max_diff_lines = 20000
show_untracked = true
ignore_dirs = ["target", "node_modules", ".git"]

[editor]
default = "nvim"

[editor.profiles.nvim]
command = ["nvim", "+{line}", "{file}"]

[editor.profiles.helix]
command = ["hx", "{file}:{line}"]

[editor.profiles.vscode]
command = ["code", "-g", "{file}:{line}"]
```

`{file}` is replaced with the absolute path of the selected file and `{line}`
with the line number, or `1` when no line is known. `ignore_dirs` entries are
not watched and do not appear in the tree.

## Keys

| Key | Action |
| --- | --- |
| `q`, `Ctrl+C` | Quit |
| `j`, `Down` | Move down |
| `k`, `Up` | Move up |
| `g` | First row |
| `G` | Last row |
| `Enter` | Expand/collapse a directory, open the diff of a file; in the diff pane, open the file in the editor |
| `Space` | Toggle directory collapse |
| `Tab` | Switch focus between tree and diff |
| `Esc` | Return focus to the tree |
| `s` | Toggle staged-only (`--cached`) vs all changes |
| `f` | Show changed files only |
| `r` | Refresh git state |
| `e` | Open the selected file in the configured editor |
| `PgUp`, `PgDn` | Scroll the diff by one viewport |
| `Ctrl+u`, `Ctrl+d` | Scroll the diff by 10 lines |
| `Home`, `End` | Jump to the start/end of the diff |

Files changed in the last 10 seconds carry a `●` marker. The tree refreshes
automatically after filesystem events (debounced by `refresh_debounce_ms`).
Pressing `Enter` twice on the same file opens it in the configured editor;
`Tab`/`Esc` returns focus to the tree.
