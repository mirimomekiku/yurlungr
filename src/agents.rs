//! Spotting coding agents by the program running in a pane's foreground, the
//! way a process list would. Hooks give better state, but this works with no
//! setup: start `claude` in a pane and it shows up in the agents list.

use std::path::Path;

/// (executable name, name shown in the sidebar)
const AGENTS: &[(&str, &str)] = &[
    ("claude", "claude"),
    ("codex", "codex"),
    ("opencode", "opencode"),
    ("gemini", "gemini"),
    ("qwen", "qwen"),
    ("kimi", "kimi"),
    ("droid", "droid"),
    ("cursor-agent", "cursor"),
    ("aider", "aider"),
    ("amp", "amp"),
    ("goose", "goose"),
    ("crush", "crush"),
    ("qodercli", "qoder"),
    ("copilot", "copilot"),
    ("grok", "grok"),
    ("kilo", "kilo"),
];

/// Agents installed from npm/pip run as `node /path/to/<package>/cli.js`, where
/// only the package directory names the agent.
const PACKAGES: &[(&str, &str)] = &[
    ("claude-code", "claude"),
    ("gemini-cli", "gemini"),
    ("qwen-code", "qwen"),
    ("opencode-ai", "opencode"),
    ("kimi-cli", "kimi"),
];

/// Programs that run a script given as an argument.
const RUNTIMES: &[&str] =
    &["node", "nodejs", "bun", "deno", "python", "python3", "ruby", "npx", "bunx", "uv", "uvx"];

fn base(arg: &str) -> String {
    let name = Path::new(arg)
        .file_name()
        .map_or_else(|| arg.to_string(), |n| n.to_string_lossy().into_owned());
    for ext in [".js", ".mjs", ".cjs", ".ts", ".py", ".exe"] {
        if let Some(stem) = name.strip_suffix(ext) {
            return stem.to_string();
        }
    }
    name
}

fn by_name(name: &str) -> Option<&'static str> {
    AGENTS.iter().find(|(exe, _)| *exe == name).map(|(_, shown)| *shown)
}

/// The agent running as `args` (a full command line), if it is a known one.
/// Only the program itself, or the script handed to a runtime, is considered:
/// `cat claude` and `vim codex.md` are not agents.
pub fn detect(args: &[String]) -> Option<&'static str> {
    let first = args.first()?;
    let program = base(first);
    if let Some(agent) = by_name(&program) {
        return Some(agent);
    }
    if !RUNTIMES.contains(&program.as_str()) {
        return None;
    }
    // Skip flags (and the module name after -m) to find the script.
    let mut rest = args[1..].iter();
    while let Some(arg) = rest.next() {
        if arg == "-m" {
            return rest.next().and_then(|m| by_name(&base(m)));
        }
        if arg.starts_with('-') {
            continue;
        }
        if let Some(agent) = by_name(&base(arg)) {
            return Some(agent);
        }
        let normalised = arg.replace('\\', "/");
        return PACKAGES
            .iter()
            .find(|(dir, _)| normalised.contains(&format!("/{dir}/")))
            .map(|(_, a)| *a);
    }
    None
}

/// The command line of a process, or `None` where it cannot be read.
pub fn command_line(pid: i64) -> Option<Vec<String>> {
    if pid <= 0 {
        return None;
    }
    if let Ok(raw) = std::fs::read(format!("/proc/{pid}/cmdline")) {
        let args: Vec<String> = raw
            .split(|b| *b == 0)
            .filter(|a| !a.is_empty())
            .map(|a| String::from_utf8_lossy(a).into_owned())
            .collect();
        return (!args.is_empty()).then_some(args);
    }
    // No /proc (macOS, BSD): ask ps. Arguments with spaces are split, which is
    // good enough to recognise the program and its script.
    let out = std::process::Command::new("ps")
        .args(["-o", "args=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!line.is_empty()).then(|| line.split_whitespace().map(String::from).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn recognises_agents_run_directly() {
        assert_eq!(detect(&args("claude")), Some("claude"));
        assert_eq!(detect(&args("/home/me/.local/bin/claude --resume")), Some("claude"));
        assert_eq!(detect(&args("codex exec fix it")), Some("codex"));
        assert_eq!(detect(&args("cursor-agent")), Some("cursor"));
        assert_eq!(detect(&args("/usr/bin/qodercli")), Some("qoder"));
    }

    #[test]
    fn recognises_agents_run_through_a_runtime() {
        assert_eq!(
            detect(&args("node /usr/lib/node_modules/@openai/codex/bin/codex.js")),
            Some("codex")
        );
        assert_eq!(
            detect(&args("node /usr/lib/node_modules/@anthropic-ai/claude-code/cli.js --continue")),
            Some("claude")
        );
        assert_eq!(
            detect(&args("node --no-warnings /opt/@google/gemini-cli/dist/index.js")),
            Some("gemini")
        );
        assert_eq!(detect(&args("python3 -m aider --model x")), Some("aider"));
        assert_eq!(
            detect(&args("bun /app/node_modules/opencode-ai/bin/opencode")),
            Some("opencode")
        );
    }

    #[test]
    fn ordinary_commands_are_not_agents() {
        for cmd in [
            "bash",
            "cat claude",
            "vim codex.md",
            "grep -r gemini .",
            "node server.js",
            "python3 script.py claude",
            "less /home/me/claude/notes",
            "ssh gemini-host",
        ] {
            assert_eq!(detect(&args(cmd)), None, "{cmd}");
        }
        assert_eq!(detect(&[]), None);
    }

    #[test]
    fn reads_our_own_command_line() {
        let me =
            command_line(i64::from(std::process::id())).expect("this process has a command line");
        assert!(!me.is_empty());
        assert_eq!(command_line(0), None);
        assert_eq!(command_line(-5), None);
    }
}
