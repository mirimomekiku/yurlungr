//! `yurlungr integration install|uninstall|status [agent]`: merge yurlungr's
//! status hooks into an agent's own hook configuration.
//!
//! Only entries that call `yurlungr hook ...` are ever added or removed; the
//! rest of the file is left as it was. A one-time backup is kept next to it.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use clap::ValueEnum;
use serde_json::{json, Value};

#[derive(Clone, Copy, ValueEnum)]
pub enum Agent {
    Claude,
    Codex,
    Cursor,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Action {
    Install,
    Uninstall,
    Status,
}

const ALL: [Agent; 3] = [Agent::Claude, Agent::Codex, Agent::Cursor];

/// How an agent lays out its `hooks` section.
#[derive(Clone, Copy)]
enum Style {
    /// `{"hooks": {"Event": [{"hooks": [{"type": "command", "command": ...}]}]}}`
    Nested,
    /// `{"version": 1, "hooks": {"event": [{"command": ...}]}}`
    Flat,
}

impl Agent {
    fn name(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::Cursor => "cursor",
        }
    }

    fn style(self) -> Style {
        match self {
            Agent::Claude | Agent::Codex => Style::Nested,
            Agent::Cursor => Style::Flat,
        }
    }

    /// (agent event, state to report)
    fn events(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Agent::Claude => &[
                ("SessionStart", "idle"),
                ("UserPromptSubmit", "working"),
                ("PreToolUse", "working"),
                ("PostToolUse", "working"),
                ("PermissionRequest", "blocked"),
                ("Stop", "idle"),
                ("SessionEnd", "release"),
            ],
            Agent::Codex => &[
                ("SessionStart", "idle"),
                ("UserPromptSubmit", "working"),
                ("PreToolUse", "working"),
                ("PostToolUse", "working"),
                ("PermissionRequest", "blocked"),
                ("Stop", "idle"),
            ],
            Agent::Cursor => &[("beforeSubmitPrompt", "working"), ("stop", "idle")],
        }
    }

    fn config_file(self) -> Result<PathBuf> {
        let (var, dir, file) = match self {
            Agent::Claude => ("CLAUDE_CONFIG_DIR", ".claude", "settings.json"),
            Agent::Codex => ("CODEX_HOME", ".codex", "hooks.json"),
            Agent::Cursor => ("CURSOR_CONFIG_DIR", ".cursor", "hooks.json"),
        };
        let base = match env::var_os(var).filter(|v| !v.is_empty()) {
            Some(v) => PathBuf::from(v),
            None => home()?.join(dir),
        };
        Ok(base.join(file))
    }
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
    cmd.to_lowercase().contains("yurlungr") && cmd.contains(" hook ")
}

fn entry_is_ours(entry: &Value, style: Style) -> bool {
    let direct = |v: &Value| v.get("command").and_then(Value::as_str).is_some_and(is_ours);
    match style {
        Style::Flat => direct(entry),
        Style::Nested => entry
            .get("hooks")
            .and_then(Value::as_array)
            .is_some_and(|hs| hs.iter().any(direct)),
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
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".yurlungr.tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, serde_json::to_string_pretty(value)? + "\n")?;
    fs::rename(&tmp, path)?;
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
                Style::Nested => {
                    json!({ "hooks": [{ "type": "command", "command": cmd, "timeout": 10 }] })
                }
                Style::Flat => json!({ "command": cmd }),
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

pub fn run(action: Action, agent: Option<Agent>) -> Result<()> {
    let agents: Vec<Agent> = agent.map_or_else(|| ALL.to_vec(), |a| vec![a]);
    let exe = env::current_exe()?;
    let mut failed = false;

    for agent in agents {
        let result = (|| -> Result<String> {
            let path = agent.config_file()?;
            let mut root = load(&path)?;
            match action {
                Action::Status => {
                    let state = if installed(&root, agent.style()) { "installed" } else { "not installed" };
                    Ok(format!("{state} ({})", path.display()))
                }
                Action::Install | Action::Uninstall => {
                    let install = matches!(action, Action::Install);
                    if !install && !path.exists() {
                        return Ok("nothing to remove".to_string());
                    }
                    edit(&mut root, agent, &exe, install)?;
                    save(&path, &root)?;
                    Ok(format!(
                        "{} {}",
                        if install { "installed into" } else { "removed from" },
                        path.display()
                    ))
                }
            }
        })();
        match result {
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
