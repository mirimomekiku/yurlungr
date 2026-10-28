//! What the settings dialog shows: the state of each agent's hooks and login,
//! which LLM provider keys are present, and where yurlungr's own files live.
//!
//! Nothing secret is ever read. Credential files are only checked for
//! existence and API keys only for whether the variable is set.

use std::{env, path::PathBuf};

use crate::{
    config,
    integration::{Agent, Auth, ALL},
    proto,
};

pub struct AgentRow {
    pub agent: Agent,
    pub binary: Option<PathBuf>,
    /// Whether our hooks are installed, or why that could not be determined.
    pub hooks: Result<bool, String>,
    pub config: Option<PathBuf>,
    pub auth: Auth,
}

pub struct ProviderRow {
    pub name: &'static str,
    /// The variable that is set, if any.
    pub set_var: Option<&'static str>,
    pub vars: &'static [&'static str],
}

/// (provider, environment variables that hold its API key)
const PROVIDERS: &[(&str, &[&str])] = &[
    ("Anthropic", &["ANTHROPIC_API_KEY"]),
    ("OpenAI", &["OPENAI_API_KEY"]),
    ("Google Gemini", &["GEMINI_API_KEY", "GOOGLE_API_KEY"]),
    ("DeepSeek", &["DEEPSEEK_API_KEY"]),
    ("Moonshot / Kimi", &["MOONSHOT_API_KEY", "KIMI_API_KEY"]),
    ("Alibaba Qwen", &["DASHSCOPE_API_KEY"]),
    ("OpenRouter", &["OPENROUTER_API_KEY"]),
    ("Mistral", &["MISTRAL_API_KEY"]),
    ("Groq", &["GROQ_API_KEY"]),
    ("xAI", &["XAI_API_KEY"]),
];

pub struct Snapshot {
    pub agents: Vec<AgentRow>,
    pub providers: Vec<ProviderRow>,
    /// Label and value lines about yurlungr itself.
    pub info: Vec<(String, String)>,
}

pub fn gather(session: &str, theme: &str, key_overrides: usize, warnings: &[String]) -> Snapshot {
    let agents = ALL
        .iter()
        .map(|&agent| AgentRow {
            agent,
            binary: agent.binary_path(),
            hooks: agent.is_installed().map_err(|e| format!("{e:#}")),
            config: agent.config_file().ok(),
            auth: agent.auth(),
        })
        .collect();

    let providers = PROVIDERS
        .iter()
        .map(|&(name, vars)| ProviderRow {
            name,
            set_var: vars.iter().copied().find(|v| env::var_os(v).is_some_and(|x| !x.is_empty())),
            vars,
        })
        .collect();

    let show =
        |p: Option<PathBuf>| p.map_or_else(|| "unknown".to_string(), |p| p.display().to_string());
    let config_path = config::path();
    let config_state = match config_path.as_ref().map(|p| p.exists()) {
        Some(true) => "present",
        Some(false) => "not created yet (defaults in use)",
        None => "unavailable",
    };
    let mut info = vec![
        ("version".to_string(), env!("CARGO_PKG_VERSION").to_string()),
        ("session".to_string(), session.to_string()),
        ("theme".to_string(), theme.to_string()),
        ("config file".to_string(), format!("{} ({config_state})", show(config_path))),
        ("custom keybindings".to_string(), key_overrides.to_string()),
        ("state dir".to_string(), show(proto::data_dir().ok())),
    ];
    for w in warnings {
        info.push(("config warning".to_string(), w.clone()));
    }
    Snapshot { agents, providers, info }
}
