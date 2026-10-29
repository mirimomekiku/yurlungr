//! `yurlungr integration install|uninstall|status [agent]`: merge yurlungr's
//! status hooks into an agent's own hook configuration.
//!
//! Only entries that call `yurlungr hook ...` are ever added or removed; the
//! rest of the file is left as it was. A one-time backup is kept next to it.
//!
//! Agents that have no hook system (or none we could verify) are not listed
//! here; their panes still get working/blocked/idle from screen detection.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use clap::ValueEnum;
use serde_json::{json, Value};

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Agent {
    Claude,
    Codex,
    Cursor,
    Qwen,
    Kimi,
    Droid,
    Qoder,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Action {
    Install,
    Uninstall,
    Status,
}

pub const ALL: [Agent; 7] = [
    Agent::Claude,
    Agent::Codex,
    Agent::Cursor,
    Agent::Qwen,
    Agent::Kimi,
    Agent::Droid,
    Agent::Qoder,
];

/// How an agent lays out its hooks.
#[derive(Clone, Copy)]
enum Style {
    /// `{"hooks": {"Event": [{"hooks": [{"type": "command", "command": ...}]}]}}`
    Nested,
    /// `{"version": 1, "hooks": {"event": [{"command": ...}]}}`
    Flat,
    /// `[[hooks]]` tables with `event` and `command` keys in a TOML file.
    Toml,
}

/// Events shared by the agents that follow Claude Code's hook model.
const CLAUDE_LIKE: &[(&str, &str)] = &[
    ("SessionStart", "idle"),
    ("UserPromptSubmit", "working"),
    ("PreToolUse", "working"),
    ("PostToolUse", "working"),
    ("Stop", "idle"),
    ("SessionEnd", "release"),
];

impl Agent {
    pub fn name(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::Cursor => "cursor",
            Agent::Qwen => "qwen",
            Agent::Kimi => "kimi",
            Agent::Droid => "droid",
            Agent::Qoder => "qoder",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Agent::Claude => "Claude Code",
            Agent::Codex => "Codex CLI",
            Agent::Cursor => "Cursor",
            Agent::Qwen => "Qwen Code",
            Agent::Kimi => "Kimi Code CLI",
            Agent::Droid => "Factory Droid",
            Agent::Qoder => "Qoder CLI",
        }
    }

    /// Executables whose presence on PATH means the agent is installed.
    fn binaries(self) -> &'static [&'static str] {
        match self {
            Agent::Claude => &["claude"],
            Agent::Codex => &["codex"],
            Agent::Cursor => &["cursor-agent", "cursor"],
            Agent::Qwen => &["qwen"],
            Agent::Kimi => &["kimi"],
            Agent::Droid => &["droid"],
            Agent::Qoder => &["qodercli"],
        }
    }

    fn style(self) -> Style {
        match self {
            Agent::Claude | Agent::Codex | Agent::Qwen | Agent::Droid | Agent::Qoder => {
                Style::Nested
            }
            Agent::Cursor => Style::Flat,
            Agent::Kimi => Style::Toml,
        }
    }

    /// (agent event, state to report)
    fn events(self) -> Vec<(&'static str, &'static str)> {
        let mut events = CLAUDE_LIKE.to_vec();
        match self {
            Agent::Claude | Agent::Qwen => events.push(("PermissionRequest", "blocked")),
            Agent::Codex => {
                events.retain(|(e, _)| *e != "SessionEnd");
                events.push(("PermissionRequest", "blocked"));
            }
            Agent::Cursor => events = vec![("beforeSubmitPrompt", "working"), ("stop", "idle")],
            Agent::Kimi | Agent::Droid | Agent::Qoder => {}
        }
        events
    }

    pub fn config_file(self) -> Result<PathBuf> {
        let (var, dir, file) = match self {
            Agent::Claude => ("CLAUDE_CONFIG_DIR", ".claude", "settings.json"),
            Agent::Codex => ("CODEX_HOME", ".codex", "hooks.json"),
            Agent::Cursor => ("CURSOR_CONFIG_DIR", ".cursor", "hooks.json"),
            Agent::Qwen => ("QWEN_HOME", ".qwen", "settings.json"),
            Agent::Droid => ("FACTORY_HOME", ".factory", "settings.json"),
            Agent::Qoder => ("QODER_CONFIG_DIR", ".qoder", "settings.json"),
            Agent::Kimi => {
                // Kimi's docs disagree on the directory; use whichever exists.
                let home = home()?;
                let dir = if home.join(".kimi-code").is_dir() || !home.join(".kimi").is_dir() {
                    ".kimi-code"
                } else {
                    ".kimi"
                };
                ("KIMI_CODE_HOME", dir, "config.toml")
            }
        };
        let base = match env::var_os(var).filter(|v| !v.is_empty()) {
            Some(v) => PathBuf::from(v),
            None => home()?.join(dir),
        };
        Ok(base.join(file))
    }

    /// Where the agent's executable is, if it is on PATH.
    pub fn binary_path(self) -> Option<PathBuf> {
        self.binaries().iter().find_map(|b| which_path(b))
    }

    /// Whether the agent has a login on this machine. Only the existence of its
    /// credentials file is checked; the file is never opened.
    pub fn auth(self) -> Auth {
        let file = match self {
            Agent::Claude => ".credentials.json",
            Agent::Codex => "auth.json",
            Agent::Qwen => "oauth_creds.json",
            _ => return Auth::Unknown,
        };
        match self.config_file().ok().and_then(|p| p.parent().map(|d| d.join(file))) {
            Some(path) if path.is_file() => Auth::LoggedIn(path),
            _ => Auth::NotFound,
        }
    }

    /// True if the agent's executable is on PATH or its config folder exists.
    pub fn detected(self) -> bool {
        let on_path = self.binaries().iter().any(|b| which(b));
        on_path
            || self.config_file().ok().and_then(|p| p.parent().map(Path::is_dir)).unwrap_or(false)
    }

    pub fn is_installed(self) -> Result<bool> {
        let path = self.config_file()?;
        if !path.exists() {
            return Ok(false);
        }
        Ok(match self.style() {
            Style::Toml => fs::read_to_string(&path)?.contains(BEGIN),
            style => installed(&load(&path)?, style),
        })
    }

    /// Install, remove or check hooks; returns a one-line description of what happened.
    pub fn apply(self, action: Action) -> Result<String> {
        let path = self.config_file()?;
        let exe = env::current_exe()?;
        match action {
            Action::Status => {
                let state = if self.is_installed()? { "installed" } else { "not installed" };
                Ok(format!("{state} ({})", path.display()))
            }
            Action::Install | Action::Uninstall => {
                let install = matches!(action, Action::Install);
                if !install && !path.exists() {
                    return Ok("nothing to remove".to_string());
                }
                match self.style() {
                    Style::Toml => {
                        let text =
                            if path.exists() { fs::read_to_string(&path)? } else { String::new() };
                        let new = edit_toml(&text, &exe, self, install)
                            .with_context(|| format!("{} left unchanged", path.display()))?;
                        save_text(&path, &new)?;
                    }
                    _ => {
                        let mut root = load(&path)?;
                        edit(&mut root, self, &exe, install)?;
                        save(&path, &root)?;
                    }
                }
                let mut note = String::new();
                if install && self == Agent::Codex {
                    note = enable_codex_hooks_feature()?;
                }
                Ok(format!(
                    "{} {}{note}",
                    if install { "installed into" } else { "removed from" },
                    path.display()
                ))
            }
        }
    }
}

/// Result of [`Agent::auth`].
pub enum Auth {
    LoggedIn(PathBuf),
    NotFound,
    /// We do not know where this agent keeps its login.
    Unknown,
}

fn which_path(bin: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .flat_map(|dir| [dir.join(bin), dir.join(format!("{bin}.exe"))])
        .find(|p| p.is_file())
}

fn which(bin: &str) -> bool {
    which_path(bin).is_some()
}

fn home() -> Result<PathBuf> {
    env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from)
        .context("cannot locate the home directory")
}

fn command(exe: &Path, agent: Agent, state: &str) -> String {
    format!("\"{}\" hook {} {}", exe.display(), agent.name(), state)
}

fn is_ours(cmd: &str) -> bool {
    // Our commands look like `"<path>/yurlungr" hook <agent> <state>`.
    let Some((exe, _)) = cmd.rsplit_once(" hook ") else { return false };
    Path::new(exe.trim().trim_matches('"'))
        .file_stem()
        .is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case("yurlungr"))
}

fn entry_is_ours(entry: &Value, style: Style) -> bool {
    let direct = |v: &Value| v.get("command").and_then(Value::as_str).is_some_and(is_ours);
    match style {
        Style::Flat => direct(entry),
        Style::Nested => {
            entry.get("hooks").and_then(Value::as_array).is_some_and(|hs| hs.iter().any(direct))
        }
        Style::Toml => false,
    }
}

fn load(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let text = fs::read_to_string(path)?;
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&text).with_context(|| {
        format!("{} is not plain JSON (comments are not supported); left unchanged", path.display())
    })
}

fn save(path: &Path, value: &Value) -> Result<()> {
    save_text(path, &(serde_json::to_string_pretty(value)? + "\n"))
}

/// Write atomically (keeping permissions and symlinks), keeping a one-time `.yurlungr.bak` of the original.
fn save_text(path: &Path, text: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    if path.exists() {
        let mut backup = path.as_os_str().to_owned();
        backup.push(".yurlungr.bak");
        let backup = PathBuf::from(backup);
        if !backup.exists() {
            fs::copy(path, &backup)?;
        }
    }
    crate::file_browser::write_atomic(path, text.as_bytes())?;
    Ok(())
}

/// Remove every yurlungr entry, then add the current ones when installing.
fn edit(root: &mut Value, agent: Agent, exe: &Path, install: bool) -> Result<()> {
    let style = agent.style();
    let obj = root.as_object_mut().context("config root is not a JSON object")?;
    if install {
        if let Style::Flat = style {
            obj.entry("version").or_insert_with(|| json!(1));
        }
    }
    let hooks = obj
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("\"hooks\" is not an object")?;

    for entries in hooks.values_mut() {
        if let Some(list) = entries.as_array_mut() {
            list.retain(|e| !entry_is_ours(e, style));
        }
    }
    if install {
        for (event, state) in agent.events() {
            let cmd = command(exe, agent, state);
            let entry = match style {
                Style::Flat => json!({ "command": cmd }),
                _ => json!({ "hooks": [{ "type": "command", "command": cmd, "timeout": 10 }] }),
            };
            hooks
                .entry(event.to_string())
                .or_insert_with(|| json!([]))
                .as_array_mut()
                .with_context(|| format!("hooks.{event} is not an array"))?
                .push(entry);
        }
    }
    hooks.retain(|_, v| v.as_array().is_none_or(|a| !a.is_empty()));
    if hooks.is_empty() {
        obj.remove("hooks");
    }
    Ok(())
}

fn installed(root: &Value, style: Style) -> bool {
    root.get("hooks").and_then(Value::as_object).is_some_and(|hooks| {
        hooks.values().filter_map(Value::as_array).flatten().any(|e| entry_is_ours(e, style))
    })
}

// ---- TOML (Kimi) -----------------------------------------------------------

const BEGIN: &str = "# >>> yurlungr hooks (managed; do not edit) >>>";
const END: &str = "# <<< yurlungr hooks <<<";

/// Drop the managed block (and the blank line before it), keeping everything else byte for byte.
fn strip_block(text: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut skipping = false;
    for line in text.lines() {
        if line.trim() == BEGIN {
            skipping = true;
            while out.last().is_some_and(|l| l.trim().is_empty()) {
                out.pop();
            }
        } else if skipping {
            skipping = line.trim() != END;
        } else {
            out.push(line);
        }
    }
    if out.is_empty() {
        String::new()
    } else {
        out.join("\n") + "\n"
    }
}

fn edit_toml(text: &str, exe: &Path, agent: Agent, install: bool) -> Result<String> {
    if !text.trim().is_empty() {
        toml::from_str::<toml::Table>(text).context("existing file is not valid TOML")?;
    }
    let mut out = strip_block(text);
    if install {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("{BEGIN}\n"));
        for (event, state) in agent.events() {
            let cmd = command(exe, agent, state);
            if cmd.contains('\'') {
                bail!("the yurlungr path contains a quote character, which is not supported here");
            }
            out.push_str(&format!(
                "[[hooks]]\nevent = \"{event}\"\ncommand = '{cmd}'\ntimeout = 10\n\n"
            ));
        }
        out.push_str(&format!("{END}\n"));
    }
    if !out.trim().is_empty() {
        toml::from_str::<toml::Table>(&out).context("the edited file would not be valid TOML")?;
    }
    Ok(out)
}

// ---- Codex feature flag -----------------------------------------------------

/// Codex ignores hooks.json unless `[features] hooks = true` is set in config.toml.
fn codex_features_edit(text: &str) -> Result<Option<String>> {
    let table: toml::Table = if text.trim().is_empty() {
        toml::Table::new()
    } else {
        toml::from_str(text).context("config.toml is not valid TOML")?
    };
    let flag =
        |t: &toml::Table| t.get("features").and_then(|f| f.get("hooks")).and_then(|v| v.as_bool());
    if flag(&table) == Some(true) {
        return Ok(None);
    }

    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    match lines.iter().position(|l| l.trim() == "[features]") {
        Some(i) => {
            let section_end = lines[i + 1..]
                .iter()
                .position(|l| l.trim_start().starts_with('['))
                .map_or(lines.len(), |n| i + 1 + n);
            let existing = (i + 1..section_end).find(|&j| {
                let l = lines[j].trim_start();
                l.starts_with("hooks") && l[5..].trim_start().starts_with('=')
            });
            match existing {
                Some(j) => lines[j] = "hooks = true".to_string(),
                None => lines.insert(i + 1, "hooks = true".to_string()),
            }
        }
        None => {
            if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push("[features]".to_string());
            lines.push("hooks = true".to_string());
        }
    }
    let new = lines.join("\n") + "\n";
    let parsed: toml::Table =
        toml::from_str(&new).context("could not enable the hooks feature safely")?;
    if flag(&parsed) != Some(true) {
        bail!("could not enable the hooks feature safely");
    }
    Ok(Some(new))
}

/// Returns a short note for the install message.
fn enable_codex_hooks_feature() -> Result<String> {
    let base = match env::var_os("CODEX_HOME").filter(|v| !v.is_empty()) {
        Some(v) => PathBuf::from(v),
        None => home()?.join(".codex"),
    };
    let path = base.join("config.toml");
    let text = if path.exists() { fs::read_to_string(&path)? } else { String::new() };
    Ok(match codex_features_edit(&text)? {
        Some(new) => {
            save_text(&path, &new)?;
            format!("; enabled [features] hooks in {}", path.display())
        }
        None => String::new(),
    })
}

pub fn run(action: Action, agent: Option<Agent>) -> Result<()> {
    let agents: Vec<Agent> = agent.map_or_else(|| ALL.to_vec(), |a| vec![a]);
    let mut failed = false;

    for agent in agents {
        match agent.apply(action) {
            Ok(msg) => println!("{:<7} {msg}", agent.name()),
            Err(e) => {
                failed = true;
                eprintln!("{:<7} error: {e:#}", agent.name());
            }
        }
    }
    if failed {
        bail!("some integrations failed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = "/opt/yurlungr/yurlungr";

    fn hooks_of(root: &Value) -> usize {
        root["hooks"]
            .as_object()
            .map_or(0, |h| h.values().map(|v| v.as_array().unwrap().len()).sum())
    }

    #[test]
    fn only_our_own_hook_commands_are_recognised() {
        assert!(is_ours(&command(Path::new(EXE), Agent::Claude, "idle")));
        assert!(is_ours("\"/opt/a hook b/yurlungr\" hook codex working"));
        assert!(!is_ours("/home/me/yurlungr/notify.sh hook deploy"));
        assert!(!is_ours("echo yurlungr"));
    }

    #[test]
    fn nested_install_is_idempotent_and_keeps_foreign_hooks() {
        let mut root = json!({
            "theme": "dark",
            "hooks": { "Stop": [{ "hooks": [{ "type": "command", "command": "notify-send done" }] }] }
        });
        let exe = Path::new(EXE);
        edit(&mut root, Agent::Qwen, exe, true).unwrap();
        let once = root.clone();
        edit(&mut root, Agent::Qwen, exe, true).unwrap();
        assert_eq!(root, once, "installing twice must not duplicate entries");
        assert!(installed(&root, Style::Nested));
        assert_eq!(root["theme"], "dark");
        assert_eq!(root["hooks"]["Stop"].as_array().unwrap().len(), 2);

        edit(&mut root, Agent::Qwen, exe, false).unwrap();
        assert_eq!(
            root,
            json!({
                "theme": "dark",
                "hooks": { "Stop": [{ "hooks": [{ "type": "command", "command": "notify-send done" }] }] }
            })
        );
    }

    #[test]
    fn uninstall_removes_empty_hooks_section() {
        let mut root = json!({});
        edit(&mut root, Agent::Claude, Path::new(EXE), true).unwrap();
        assert!(hooks_of(&root) >= 6);
        edit(&mut root, Agent::Claude, Path::new(EXE), false).unwrap();
        assert_eq!(root, json!({}));
    }

    #[test]
    fn cursor_uses_flat_entries_with_version() {
        let mut root = json!({});
        edit(&mut root, Agent::Cursor, Path::new(EXE), true).unwrap();
        assert_eq!(root["version"], 1);
        assert!(root["hooks"]["stop"][0]["command"]
            .as_str()
            .unwrap()
            .ends_with("hook cursor idle"));
    }

    #[test]
    fn only_claude_like_agents_with_permission_events_report_blocked() {
        let blocked = |a: Agent| a.events().iter().any(|(_, s)| *s == "blocked");
        assert!(blocked(Agent::Claude) && blocked(Agent::Qwen) && blocked(Agent::Codex));
        assert!(!blocked(Agent::Kimi) && !blocked(Agent::Droid));
    }

    #[test]
    fn kimi_toml_block_round_trips() {
        let original = "default_model = \"kimi\"\n\n[loop_control]\nmax_steps = 10\n";
        let exe = Path::new(EXE);
        let installed = edit_toml(original, exe, Agent::Kimi, true).unwrap();
        assert!(installed.starts_with(original));
        assert!(installed.contains("event = \"Stop\""));
        let parsed: toml::Table = toml::from_str(&installed).unwrap();
        assert_eq!(parsed["hooks"].as_array().unwrap().len(), Agent::Kimi.events().len());

        assert_eq!(edit_toml(&installed, exe, Agent::Kimi, true).unwrap(), installed, "idempotent");
        assert_eq!(edit_toml(&installed, exe, Agent::Kimi, false).unwrap(), original);
    }

    #[test]
    fn kimi_toml_rejects_invalid_input_and_empty_file_works() {
        assert!(edit_toml("not = [valid", Path::new(EXE), Agent::Kimi, true).is_err());
        let fresh = edit_toml("", Path::new(EXE), Agent::Kimi, true).unwrap();
        assert_eq!(edit_toml(&fresh, Path::new(EXE), Agent::Kimi, false).unwrap(), "");
    }

    #[test]
    fn codex_feature_flag_cases() {
        let enabled = codex_features_edit("[features]\nhooks = true\n").unwrap();
        assert!(enabled.is_none());

        let fresh = codex_features_edit("model = \"gpt\"\n").unwrap().unwrap();
        assert_eq!(fresh, "model = \"gpt\"\n\n[features]\nhooks = true\n");

        let in_section =
            codex_features_edit("[features]\nother = 1\n\n[mcp]\nx = 1\n").unwrap().unwrap();
        assert_eq!(in_section, "[features]\nhooks = true\nother = 1\n\n[mcp]\nx = 1\n");

        let flipped = codex_features_edit("[features]\nhooks = false\n").unwrap().unwrap();
        assert_eq!(flipped, "[features]\nhooks = true\n");

        assert!(codex_features_edit("broken = [").is_err());
        assert_eq!(codex_features_edit("").unwrap().unwrap(), "[features]\nhooks = true\n");
    }
}
