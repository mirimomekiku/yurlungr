//! Workspaces: project-level containers, each with its own panes and layout,
//! plus the agent states the sidebar rolls up across them.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::layout::{Node, PaneId};

/// One workspace. For the active workspace the live copies of `tree`, `focus`
/// and `zoomed` are kept on `App`; this holds the stored copy.
#[derive(Clone)]
pub struct Workspace {
    pub name: String,
    /// New terminals, file browsers and graphs in this workspace start here.
    pub root: PathBuf,
    pub tree: Node,
    pub focus: PaneId,
    pub zoomed: Option<PaneId>,
}

/// What a workspace looks like on disk.
#[derive(Serialize, Deserialize)]
pub struct SavedWorkspace {
    pub name: String,
    pub root: PathBuf,
    pub tree: Node,
    pub focus: PaneId,
}

/// What an agent in a pane is doing. `Done` means it finished while you were
/// looking elsewhere; it turns `Idle` once you have seen the pane.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AgentState {
    Working,
    Blocked,
    Done,
    Idle,
}

impl AgentState {
    /// Higher is more urgent: blocked, then unseen finished work, then working, then idle.
    fn urgency(self) -> u8 {
        match self {
            AgentState::Blocked => 3,
            AgentState::Done => 2,
            AgentState::Working => 1,
            AgentState::Idle => 0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AgentState::Working => "working",
            AgentState::Blocked => "blocked",
            AgentState::Done => "done",
            AgentState::Idle => "idle",
        }
    }

    /// The most urgent of several states, or `None` if there are none.
    pub fn rollup(states: impl IntoIterator<Item = AgentState>) -> Option<AgentState> {
        states.into_iter().max_by_key(|s| s.urgency())
    }
}

/// A default name from a directory, made unique among `existing`.
pub fn default_name(root: &Path, existing: &[&str]) -> String {
    let base = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "workspace".to_string());
    unique_name(&base, existing)
}

pub fn unique_name(base: &str, existing: &[&str]) -> String {
    if !existing.contains(&base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base} {n}"))
        .find(|candidate| !existing.contains(&candidate.as_str()))
        .expect("an unbounded range always yields a free name")
}

/// Trim and validate a name typed by the user.
pub fn clean_name(raw: &str) -> Result<String, &'static str> {
    let name: String = raw.trim().chars().filter(|c| !c.is_control()).collect();
    match name.chars().count() {
        0 => Err("a workspace needs a name"),
        n if n > 40 => Err("keep the name to 40 characters"),
        _ => Ok(name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use AgentState::*;

    #[test]
    fn rollup_prefers_the_most_urgent_state() {
        assert_eq!(AgentState::rollup([]), None);
        assert_eq!(AgentState::rollup([Idle]), Some(Idle));
        assert_eq!(AgentState::rollup([Idle, Working]), Some(Working));
        assert_eq!(AgentState::rollup([Working, Done, Idle]), Some(Done));
        assert_eq!(AgentState::rollup([Done, Blocked, Working]), Some(Blocked));
        assert_eq!(AgentState::rollup([Idle, Idle]), Some(Idle));
    }

    #[test]
    fn names_are_made_unique() {
        assert_eq!(unique_name("api", &[]), "api");
        assert_eq!(unique_name("api", &["api"]), "api 2");
        assert_eq!(unique_name("api", &["api", "api 2"]), "api 3");
        assert_eq!(default_name(Path::new("/home/me/yurlungr"), &["web"]), "yurlungr");
        assert_eq!(default_name(Path::new("/home/me/yurlungr"), &["yurlungr"]), "yurlungr 2");
        assert_eq!(default_name(Path::new("/"), &[]), "workspace");
    }

    #[test]
    fn typed_names_are_validated() {
        assert_eq!(clean_name("  web  "), Ok("web".to_string()));
        assert_eq!(clean_name("a\tb\n"), Ok("ab".to_string()));
        assert!(clean_name("   ").is_err());
        assert!(clean_name(&"x".repeat(41)).is_err());
        assert!(clean_name(&"x".repeat(40)).is_ok());
    }
}
