# yurlungr

A terminal workspace multiplexer in Rust: persistent PTY panes in a split layout, a git graph pane, a file browser pane, and agent status hooks. **This is the user's own variant/re-implementation of their `herdr` app** (installed locally at `~/.local/bin/herdr`, state in `~/.config/herdr`). Keep concepts parallel to herdr where reasonable (session server + thin client, agent hook integration, `*_ENV`/`*_PANE_ID`/`*_SESSION` env vars), but yurlungr is its own codebase. Do not read or modify the user's herdr config/state.

## Commands

```sh
cargo build                 # debug build
cargo run -- [path]         # attach to session "default" (starts the server if needed)
cargo run -- -s work        # named session; Ctrl+B d detaches, shells keep running
cargo run -- kill           # stop a session's server and shells
cargo fmt                   # uses rustfmt.toml (2021 style edition, Max heuristics)
cargo clippy --all-targets
cargo test                  # no tests exist yet
```

Edition 2024, so let-chains are available (clippy's `collapsible_if` warnings suggest them).

## Architecture

Client/server over loopback TCP, newline-delimited JSON (`src/proto.rs`).

- `main.rs`: clap CLI. Subcommands: `server` (hidden), `kill`, `hook <agent> <state>`, `integration install|uninstall|status [agent]`.
- `server.rs`: headless server. Owns `App`, renders the whole UI into a ratatui `TestBackend` every 30ms tick, and sends cell diffs to the single attached client. A new client replaces the old one (`Detached`).
- `client.rs`: thin client. Paints `ServerMsg::Update` cells, forwards crossterm events, relays OSC 52 clipboard payloads. Spawns the server as a detached process if none is running.
- `app.rs`: all state and input handling: pane map, layout tree, focus/zoom, tmux-style `Ctrl+B` prefix keys, mouse (selection, divider drag, context menu, passthrough to apps that request mouse reporting), layout persistence, agent report intake.
- `layout.rs`: binary split tree (`Node`), divider hit-testing, resize nudging.
- `terminal.rs`: PTY (`portable-pty`) + VT emulation (`vt100`). Status is hook-reported first, screen heuristics (output recency, prompt patterns) second.
- `views.rs`, `ui.rs`: `Pane` enum (Terminal/Graph/Files) and rendering via ratatui + `tui-term`.
- `git_graph.rs` (git2 lane graph), `file_browser.rs` (`ignore` walker), `menu.rs`, `mouse.rs` (mouse encoding), `keys.rs` (key to bytes).
- `hook.rs` + `integration.rs`: agents call `yurlungr hook <agent> <state>` using `YURLUNGR_PANE_ID`/`YURLUNGR_SESSION`; `integration` merges those hooks into Claude (`~/.claude/settings.json`), Codex and Cursor configs, with a one-time `.yurlungr.bak` backup.

State lives in `$XDG_STATE_HOME/yurlungr` (or `~/.local/state/yurlungr`, `%LOCALAPPDATA%` on Windows): `<session>.conn.json` (port, token, pid; mode 0600) and `<session>.layout.json`. Processes do not survive a server restart; a restored layout respawns fresh shells.

## Known gaps (from the initial audit)

- No tests. Good first targets: `layout::Node` (split/remove/nudge), `mouse::encode`, `keys::key_bytes`, `integration::edit` (pure JSON transforms).
- Wire format sends every changed `Cell` as JSON; fine locally, heavy for full redraws. The server also redraws every 30ms even when idle.
- Session token comes from `RandomState` hashing, not an OS CSPRNG. The conn file is chmod'd 0600 after being written, leaving a short window under a permissive umask, and Windows gets no ACL. Any local process that can read the file can drive the shells.
- `Terminal::drop` kills only the direct child, not its process group.
- File browser is read-only (no open/edit/preview action). Git graph is a snapshot (manual `r` refresh).
- `looks_blocked` prompt heuristics are English-only substring matches.
