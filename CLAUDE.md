# yurlungr

A terminal workspace multiplexer in Rust: persistent PTY panes in a split layout, a git graph pane, a file browser pane, and agent status hooks. **This is the user's own variant/re-implementation of their `herdr` app** (installed locally at `~/.local/bin/herdr`, state in `~/.config/herdr`). Keep concepts parallel to herdr where reasonable (session server + thin client, agent hook integration, `*_ENV`/`*_PANE_ID`/`*_SESSION` env vars), but yurlungr is its own codebase. Do not read or modify the user's herdr config/state.

## Commands

```sh
cargo build                 # debug build
cargo run -- [path]         # attach to session "default" (starts the server if needed)
cargo run -- -s work        # named session; Ctrl+B d detaches, shells keep running
cargo run -- kill           # stop a session's server and shells
cargo run -- setup [--yes]  # find installed agents, offer to install status hooks
cargo fmt                   # uses rustfmt.toml (2021 style edition, Max heuristics)
cargo clippy --all-targets
cargo test                  # unit tests: keymap, config, theme, layout, file ops, editor, hook installers
```

Edition 2024, so let-chains are available (clippy's `collapsible_if` warnings suggest them).

## Architecture

Client/server over loopback TCP, newline-delimited JSON (`src/proto.rs`).

- `main.rs`: clap CLI. Subcommands: `server` (hidden), `kill`, `hook <agent> <state>`, `integration install|uninstall|status [agent]`, `setup [--yes]`. First interactive run triggers `onboarding` once (marker file `onboarded` in the state dir).
- `server.rs`: headless server. Owns `App`, renders the whole UI into a ratatui `TestBackend` every 30ms tick, and sends cell diffs to the single attached client. A new client replaces the old one (`Detached`).
- `client.rs`: thin client. Paints `ServerMsg::Update` cells, forwards crossterm events, relays OSC 52 clipboard payloads. Spawns the server as a detached process if none is running.
- `app.rs`: all state and input handling (keys resolve through `Keymap`, then run as `Action`s): pane map, layout tree, focus/zoom, tmux-style `Ctrl+B` prefix keys, mouse (selection, divider drag, context menu, passthrough to apps that request mouse reporting), layout persistence, agent report intake.
- `layout.rs`: binary split tree (`Node`), divider hit-testing, resize nudging.
- `terminal.rs`: PTY (`portable-pty`) + VT emulation (`vt100`). Status is hook-reported first, screen heuristics (output recency, prompt patterns) second.
- `views.rs`, `ui.rs`: `Pane` enum (Terminal/Graph/Files/Editor) and rendering via ratatui + `tui-term`. `ui::draw` also renders the sticky bottom **dock** (one entry per pane with status dot, plus `+ new` / `≡ menu` buttons) and publishes its click targets in `App::dock`.
- `file_browser.rs`: directory listing plus the file operations (create/rename/move/copy/delete). They never overwrite; copy picks a free name; delete never follows symlinks. `views::FilesView` owns the keys and the inline prompt; the copy/cut marker (`App::file_clip`) is shared by all browsers.
- `editor.rs`: modal viewer/editor pane (opens read-only; `e` edits, `Esc` stops, `Ctrl+S` saves). Refuses binary/non-UTF-8 files and files over 2 MB, preserves CRLF and the trailing newline, and draws control characters as `·` so file contents can never inject terminal escapes. No undo, no syntax highlighting.
- `git_graph.rs` (git2 lane graph), `menu.rs`, `mouse.rs` (mouse encoding), `keys.rs` (key to bytes).
- `hook.rs` + `integration.rs` + `onboarding.rs`: agents call `yurlungr hook <agent> <state>` using `YURLUNGR_PANE_ID`/`YURLUNGR_SESSION`. `integration` merges those hooks into Claude, Codex (also sets `[features] hooks = true`), Cursor, Qwen Code, Droid, Qoder (JSON) and Kimi (TOML `[[hooks]]` between marker comments), with a one-time `.yurlungr.bak` backup. Hook commands embed the path of the running binary, so install from a stable path (`cargo install --path .`), not `target/debug`. Agents with no verified hook system (DeepSeek CLIs, Gemini CLI) rely on screen heuristics.

### Keys, config, themes, dialogs

- `keymap.rs`: every bindable `Action`, its scope (`Global`, `Files`, `Editor`), defaults, and lookup. A `Chord` is a direct key (`ctrl+right`) or `prefix <key>`. `Keymap::check` refuses prefix-key clashes, conflicts within overlapping scopes, and plain printable keys as direct global bindings (they would block typing). Key presses are normalised through `KeyCombo::new` (shift is baked into characters, `shift+tab` is `backtab`).
- Defaults: `ctrl+left/right/down` split (there is deliberately no split-up), `alt+1..9` jump to the n-th pane in dock order (works in any terminal, though some emulators reserve Alt+digit for their own tabs), `prefix <digit>` as an alternative, `ctrl+tab` / `ctrl+shift+tab` for next/previous pane, `prefix m` menu, `prefix ?` keybindings, `prefix ,` settings, `prefix t` themes. Ctrl+Tab only reaches the app if the terminal supports the kitty keyboard protocol, which the client enables (`DISAMBIGUATE_ESCAPE_CODES`); many emulators (GNOME Terminal, Konsole) reserve Ctrl+Tab for their own tabs and never send it, so use `prefix o` or rebind there; `ctrl+left/right` also shadow word-jumping in shells, which is why everything is rebindable.
- Still fixed (not configurable): list/cursor navigation (j/k, arrows, PgUp/PgDn), `Enter` to open in the file browser, prompt text entry, menu navigation, and `Esc` cannot be captured as a new binding in the dialog (edit the file instead).
- `config.rs`: `$XDG_CONFIG_HOME/yurlungr/config.toml` (`~/.config/yurlungr/`): `theme`, `[colors]` overrides, `[keys]` (only differences from defaults; `[]` means unbound). The server polls its mtime once a second and hot-reloads; a broken file falls back to defaults with warnings shown in Settings. Saving from the dialogs rewrites the file (other tables are kept, comments are not).
- `theme.rs`: 8 built-in themes (`default` uses ANSI colours; the rest are truecolor) over 16 semantic colour slots. Themes colour yurlungr's chrome only, never what programs draw in terminal panes.
- `overlay.rs`: modal dialogs. They are opened from the dock's `≡ menu` dropdown (`Menu::app`, distinct from the right-click pane menu) or by their key bindings: Keybindings (replace/add/prefix-add/clear/reset; captures the next key), Settings (per-agent binary/hooks/login status with install/uninstall, provider API-key variables, file locations), Theme picker (live preview, Esc reverts). While one is open it consumes all keys; the mouse wheel scrolls it.
- `settings.rs`: gathers the Settings snapshot. It only checks that credential files exist and that API-key variables are set; secrets are never read. Login detection covers Claude, Codex and Qwen only; other agents show "n/a". Provider variables are those of the session server's environment (captured when the server started).

State lives in `$XDG_STATE_HOME/yurlungr` (or `~/.local/state/yurlungr`, `%LOCALAPPDATA%` on Windows): `<session>.conn.json` (port, token, pid; mode 0600) and `<session>.layout.json`. Processes do not survive a server restart; a restored layout respawns fresh shells.

## Known gaps (from the initial audit)

- Tests cover `file_browser`, `editor` and `integration`/`onboarding` parsing only. Still untested: `layout::Node`, `mouse::encode`, `keys::key_bytes`, and everything that needs a PTY or a client.
- Wire format sends every changed `Cell` as JSON; fine locally, heavy for full redraws. The server also redraws every 30ms even when idle.
- Session token comes from `RandomState` hashing, not an OS CSPRNG. The conn file is chmod'd 0600 after being written, leaving a short window under a permissive umask, and Windows gets no ACL. Any local process that can read the file can drive the shells.
- `Terminal::drop` kills only the direct child, not its process group.
- Git graph is a snapshot (manual `r` refresh). The dock shows panes, not separate workspaces; there is only one layout per session. Quitting a session (`Ctrl+B q`) does not warn about unsaved editor buffers.
- `looks_blocked` prompt heuristics are English-only substring matches.
