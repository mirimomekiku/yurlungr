use std::{
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
    time::{Duration, Instant, SystemTime},
};

use anyhow::Result;
use base64::{engine::general_purpose::STANDARD, Engine};
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::{layout::Rect, widgets::Block};
use serde::{Deserialize, Serialize};

use crate::{
    config::{self, Config},
    editor::{EditorView, Mode},
    keymap::{Action, KeyCombo, Scope},
    keys::key_bytes,
    layout::{Dir, Divider, Node, PaneId},
    menu::{Menu, MenuAction},
    mouse,
    overlay::{Confirm, ConfirmAction, Keybinds, NameInput, Overlay, ThemePicker},
    proto::ReportState,
    settings,
    settings_dialog::SettingsDialog,
    terminal::{Status, Terminal},
    theme::{self, Theme},
    views::{Clip, FilesAct, FilesView, GraphAct, GraphView, Pane},
    workspace::{self, AgentState, SavedWorkspace, Workspace},
};

/// How often the config file is checked for outside edits.
const CONFIG_POLL: Duration = Duration::from_secs(1);

#[derive(Clone, Serialize, Deserialize)]
pub enum Kind {
    Terminal,
    Graph,
    Files,
    Editor(PathBuf),
}

fn kind_of(p: &Pane) -> Kind {
    match p {
        Pane::Terminal(_) => Kind::Terminal,
        Pane::Graph(_) => Kind::Graph,
        Pane::Files(_) => Kind::Files,
        Pane::Editor(v) => Kind::Editor(v.path.clone()),
    }
}

/// A clickable region of the sidebar.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DockHit {
    /// A workspace in the spaces list (index).
    Workspace(usize),
    /// An agent in the agents list: focus its pane.
    Agent(PaneId),
    NewWorkspace,
    Menu,
}

/// One row of the agents list.
pub struct AgentEntry {
    pub pane: PaneId,
    /// Index of the workspace the pane lives in.
    pub workspace: usize,
    pub name: String,
    pub state: AgentState,
}

/// What is written to disk so a new server can rebuild the layout. Processes
/// do not survive a server restart; each terminal pane gets a fresh shell.
#[derive(Serialize, Deserialize)]
struct Saved {
    root: PathBuf,
    kinds: Vec<(PaneId, Kind)>,
    next_id: PaneId,
    #[serde(default)]
    workspaces: Vec<SavedWorkspace>,
    #[serde(default)]
    active: usize,
    /// Layouts saved before workspaces existed held a single tree and focus.
    #[serde(default)]
    tree: Option<Node>,
    #[serde(default)]
    focus: PaneId,
}

struct Drag {
    path: Vec<bool>,
    dir: Dir,
    area: Rect,
}

/// A text selection in a terminal pane, in pane-relative (row, col) cells.
pub struct Selection {
    pub pane: PaneId,
    pub start: (u16, u16),
    pub end: (u16, u16),
}

impl Selection {
    /// (start, end) in reading order; `end` is inclusive.
    pub fn ordered(&self) -> ((u16, u16), (u16, u16)) {
        if self.start <= self.end {
            (self.start, self.end)
        } else {
            (self.end, self.start)
        }
    }
}

/// Environment that lets an agent's hooks find their pane and session.
fn pane_env(session: &str, id: PaneId) -> Vec<(String, String)> {
    let mut env = vec![
        ("YURLUNGR_ENV".to_string(), "1".to_string()),
        ("YURLUNGR_PANE_ID".to_string(), id.to_string()),
        ("YURLUNGR_SESSION".to_string(), session.to_string()),
    ];
    if let Ok(exe) = std::env::current_exe() {
        env.push(("YURLUNGR_BIN_PATH".to_string(), exe.display().to_string()));
    }
    env
}

fn contains(r: Rect, x: u16, y: u16) -> bool {
    x >= r.x && x < r.right() && y >= r.y && y < r.bottom()
}

fn inner(rect: Rect) -> Rect {
    Block::bordered().inner(rect)
}

pub struct App {
    root: PathBuf,
    session: String,
    pub panes: HashMap<PaneId, Pane>,
    /// All workspaces. The entry for the active one is stale: its live layout
    /// is `tree`, `focus` and `zoomed` below (see `store`).
    pub workspaces: Vec<Workspace>,
    pub active: usize,
    tree: Node,
    next_id: PaneId,
    pub focus: PaneId,
    pub zoomed: Option<PaneId>,
    /// Agent panes that finished while another pane had focus.
    done: HashSet<PaneId>,
    prev_state: HashMap<PaneId, Status>,
    /// True after the prefix key until the next key press.
    pub prefix: bool,
    pub menu: Option<Menu>,
    /// The open dialog (keybindings, settings or theme picker), if any.
    pub overlay: Option<Overlay>,
    pub config: Config,
    pub theme: Theme,
    config_mtime: Option<SystemTime>,
    config_checked: Instant,
    pub notice: Option<String>,
    pub rects: Vec<(PaneId, Rect)>,
    /// Click targets of the bottom dock, filled in by the renderer.
    pub dock: Vec<(Rect, DockHit)>,
    /// Copy/cut marker shared by all file browsers.
    pub file_clip: Option<Clip>,
    dividers: Vec<Divider>,
    drag: Option<Drag>,
    pub sel: Option<Selection>,
    selecting: Option<PaneId>,
    pub area: Rect,
    /// Base64 clipboard payloads waiting to be sent to the client (OSC 52).
    pub outbox: Vec<String>,
    pub detach: bool,
    pub quit: bool,
    layout_file: Option<PathBuf>,
    last_saved: String,
}

impl App {
    fn blank(root: PathBuf, layout_file: Option<PathBuf>, session: &str) -> Self {
        let mut config = Config::load();
        let (theme, warnings) = theme::resolve(&config.theme, &config.colors);
        config.warnings.extend(warnings);
        Self {
            overlay: None,
            config,
            theme,
            config_mtime: config::modified(),
            config_checked: Instant::now(),
            root: root.clone(),
            session: session.to_string(),
            panes: HashMap::new(),
            workspaces: vec![Workspace {
                name: workspace::default_name(&root, &[]),
                root: root.clone(),
                tree: Node::Leaf(0),
                focus: 0,
                zoomed: None,
            }],
            active: 0,
            tree: Node::Leaf(0),
            next_id: 1,
            focus: 0,
            zoomed: None,
            done: HashSet::new(),
            prev_state: HashMap::new(),
            prefix: false,
            menu: None,
            notice: None,
            rects: Vec::new(),
            dock: Vec::new(),
            file_clip: None,
            dividers: Vec::new(),
            drag: None,
            sel: None,
            selecting: None,
            area: Rect::default(),
            outbox: Vec::new(),
            detach: false,
            quit: false,
            layout_file,
            last_saved: String::new(),
        }
    }

    /// Rebuild the saved layout if there is one, otherwise start with one shell.
    pub fn new_or_restore(root: PathBuf, layout_file: PathBuf, session: &str) -> Result<Self> {
        if let Some(app) = Self::restore(&layout_file, session) {
            return Ok(app);
        }
        let mut app = Self::blank(root.clone(), Some(layout_file), session);
        let first = Terminal::spawn(&root, 24, 80, &pane_env(session, 0))?;
        app.panes.insert(0, Pane::Terminal(first));
        Ok(app)
    }

    fn restore(layout_file: &PathBuf, session: &str) -> Option<Self> {
        let saved: Saved = serde_json::from_str(&fs::read_to_string(layout_file).ok()?).ok()?;
        let mut app = Self::blank(saved.root.clone(), Some(layout_file.clone()), session);
        app.next_id = saved.next_id;

        let mut saved_workspaces = saved.workspaces;
        if saved_workspaces.is_empty() {
            saved_workspaces.push(SavedWorkspace {
                name: workspace::default_name(&saved.root, &[]),
                root: saved.root.clone(),
                tree: saved.tree?,
                focus: saved.focus,
            });
        }
        let kinds: HashMap<PaneId, Kind> = saved.kinds.into_iter().collect();
        for w in &saved_workspaces {
            let mut ids = Vec::new();
            w.tree.ids(&mut ids);
            if ids.iter().any(|id| !kinds.contains_key(id)) {
                return None;
            }
        }

        let mut restored = Vec::new();
        for w in saved_workspaces {
            let mut tree = w.tree;
            let mut ids = Vec::new();
            tree.ids(&mut ids);
            let mut alive = ids.len();
            for id in ids {
                let pane = match kinds[&id].clone() {
                    Kind::Terminal => Terminal::spawn(&w.root, 24, 80, &pane_env(session, id))
                        .ok()
                        .map(Pane::Terminal),
                    Kind::Graph => Some(Pane::Graph(Box::new(GraphView::new(&w.root)))),
                    Kind::Files => FilesView::new(w.root.clone()).ok().map(Pane::Files),
                    Kind::Editor(path) => EditorView::open(&path).ok().map(Pane::Editor),
                };
                match pane {
                    Some(p) => {
                        app.panes.insert(id, p);
                    }
                    None => {
                        // Keep at least one pane; a workspace with none is dropped below.
                        if alive > 1 {
                            tree.remove(id);
                        }
                        alive -= 1;
                    }
                }
            }
            let mut left = Vec::new();
            tree.ids(&mut left);
            left.retain(|id| app.panes.contains_key(id));
            let Some(&first) = left.first() else { continue };
            let focus = if left.contains(&w.focus) { w.focus } else { first };
            restored.push(Workspace { name: w.name, root: w.root, tree, focus, zoomed: None });
        }
        if restored.is_empty() {
            return None;
        }
        app.active = saved.active.min(restored.len() - 1);
        app.workspaces = restored;
        app.load_active();
        Some(app)
    }

    /// Write the layout to disk when it changed.
    pub fn persist(&mut self) {
        self.store();
        let Some(path) = &self.layout_file else { return };
        let mut kinds: Vec<(PaneId, Kind)> =
            self.panes.iter().map(|(id, p)| (*id, kind_of(p))).collect();
        kinds.sort_by_key(|(id, _)| *id);
        let saved = Saved {
            root: self.root.clone(),
            kinds,
            next_id: self.next_id,
            workspaces: self
                .workspaces
                .iter()
                .map(|w| SavedWorkspace {
                    name: w.name.clone(),
                    root: w.root.clone(),
                    tree: w.tree.clone(),
                    focus: w.focus,
                })
                .collect(),
            active: self.active,
            tree: None,
            focus: self.focus,
        };
        if let Ok(json) = serde_json::to_string(&saved) {
            if json != self.last_saved
                && crate::file_browser::write_atomic(path, json.as_bytes()).is_ok()
            {
                self.last_saved = json;
            }
        }
    }

    pub fn forget_layout(&self) {
        if let Some(path) = &self.layout_file {
            let _ = fs::remove_file(path);
        }
    }

    /// Apply a state reported by an agent hook. Agent names come from another
    /// process, so they are reduced to a safe, short token before display.
    pub fn report(&mut self, pane: PaneId, agent: &str, state: ReportState) {
        let agent: String = agent
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            .take(24)
            .collect();
        if let Some(Pane::Terminal(t)) = self.panes.get_mut(&pane) {
            t.set_report(agent, state);
        }
    }

    pub fn status_counts(&self) -> (usize, usize, usize) {
        let (mut working, mut blocked, mut idle) = (0, 0, 0);
        for status in self.panes.values().filter_map(Pane::status) {
            match status {
                Status::Working => working += 1,
                Status::Blocked => blocked += 1,
                Status::Idle => idle += 1,
            }
        }
        (working, blocked, idle)
    }

    /// Compute pane rectangles for the current tree inside `area`.
    pub fn relayout(&mut self, area: Rect) {
        self.rects.clear();
        self.dividers.clear();
        match self.zoomed.filter(|z| self.panes.contains_key(z)) {
            Some(z) => self.rects.push((z, area)),
            None => {
                self.zoomed = None;
                self.tree.layout(area, &mut Vec::new(), &mut self.rects, &mut self.dividers);
            }
        }
    }

    // ---- workspaces ------------------------------------------------------

    /// Copy the live layout into the active workspace's stored entry.
    fn store(&mut self) {
        if let Some(w) = self.workspaces.get_mut(self.active) {
            w.tree = self.tree.clone();
            w.focus = self.focus;
            w.zoomed = self.zoomed;
        }
    }

    /// Make the active workspace's stored layout the live one.
    fn load_active(&mut self) {
        let w = &self.workspaces[self.active];
        self.tree = w.tree.clone();
        self.focus = w.focus;
        self.zoomed = w.zoomed;
    }

    /// Forget transient state that belongs to the workspace being left.
    fn reset_interaction(&mut self) {
        self.sel = None;
        self.selecting = None;
        self.drag = None;
        self.rects.clear();
        self.dividers.clear();
    }

    pub fn ws_root(&self) -> PathBuf {
        self.workspaces[self.active].root.clone()
    }

    /// Pane ids of workspace `i`, in layout order.
    pub fn ws_ids(&self, i: usize) -> Vec<PaneId> {
        let mut ids = Vec::new();
        match self.workspaces.get(i) {
            Some(_) if i == self.active => self.tree.ids(&mut ids),
            Some(w) => w.tree.ids(&mut ids),
            None => {}
        }
        ids
    }

    fn workspace_of(&self, pane: PaneId) -> Option<usize> {
        (0..self.workspaces.len()).find(|&i| self.ws_ids(i).contains(&pane))
    }

    pub fn switch_workspace(&mut self, i: usize) {
        if i >= self.workspaces.len() || i == self.active {
            return;
        }
        self.store();
        self.active = i;
        self.load_active();
        self.reset_interaction();
    }

    fn goto_workspace(&mut self, n: u8) {
        let i = n as usize - 1;
        if i < self.workspaces.len() {
            self.switch_workspace(i);
        } else {
            self.notice = Some(format!("there is no workspace {n}"));
        }
    }

    /// Jump to the n-th pane of the current workspace (1-based, layout order).
    fn goto_pane(&mut self, n: u8) {
        match self.pane_order().get(n as usize - 1) {
            Some(&id) => {
                self.focus = id;
                if self.zoomed.is_some() {
                    self.zoomed = Some(id);
                }
                self.sel = None;
            }
            None => self.notice = Some(format!("there is no pane {n} in this workspace")),
        }
    }

    fn cycle_workspace(&mut self, back: bool) {
        let n = self.workspaces.len();
        self.switch_workspace((self.active + if back { n - 1 } else { 1 }) % n);
    }

    /// Focus a pane in any workspace, switching to that workspace first.
    pub fn focus_pane(&mut self, id: PaneId) {
        let Some(ws) = self.workspace_of(id) else { return };
        self.switch_workspace(ws);
        self.focus = id;
        if self.zoomed.is_some() {
            self.zoomed = Some(id);
        }
        self.sel = None;
    }

    /// Where a new workspace starts: the folder a file browser is showing, else
    /// the current workspace's root.
    fn suggested_root(&self) -> PathBuf {
        match self.panes.get(&self.focus) {
            Some(Pane::Files(v)) => v.fb.dir.clone(),
            _ => self.ws_root(),
        }
    }

    pub fn new_workspace(&mut self) {
        let root = self.suggested_root();
        let id = self.next_id;
        let terminal = match Terminal::spawn(&root, 24, 80, &pane_env(&self.session, id)) {
            Ok(t) => t,
            Err(e) => {
                self.notice = Some(format!("failed to start shell: {e}"));
                return;
            }
        };
        self.next_id += 1;
        self.store();
        let names: Vec<&str> = self.workspaces.iter().map(|w| w.name.as_str()).collect();
        let name = workspace::default_name(&root, &names);
        self.panes.insert(id, Pane::Terminal(terminal));
        self.workspaces.push(Workspace {
            name,
            root,
            tree: Node::Leaf(id),
            focus: id,
            zoomed: None,
        });
        self.active = self.workspaces.len() - 1;
        self.load_active();
        self.reset_interaction();
    }

    /// Drop a workspace and every pane in it (their shells are killed).
    fn remove_workspace(&mut self, i: usize) {
        for id in self.ws_ids(i) {
            self.panes.remove(&id);
            self.done.remove(&id);
            self.prev_state.remove(&id);
        }
        self.workspaces.remove(i);
        if i == self.active {
            self.active = i.min(self.workspaces.len() - 1);
            self.load_active();
            self.reset_interaction();
        } else if i < self.active {
            self.active -= 1;
        }
    }

    /// Ask before closing a workspace: it ends every shell in it.
    fn request_close_workspace(&mut self, i: usize) {
        if self.workspaces.len() <= 1 {
            self.notice =
                Some("this is the only workspace; detach or quit the session instead".into());
            return;
        }
        let Some(w) = self.workspaces.get(i) else { return };
        let panes = self.ws_ids(i);
        let shells =
            panes.iter().filter(|id| matches!(self.panes.get(id), Some(Pane::Terminal(_)))).count();
        let mut lines = vec![format!(
            "'{}' has {} pane{} and {} shell{}.",
            w.name,
            panes.len(),
            if panes.len() == 1 { "" } else { "s" },
            shells,
            if shells == 1 { "" } else { "s" }
        )];
        lines.push("Everything running in them will be stopped.".to_string());
        let unsaved = panes
            .iter()
            .filter(|id| matches!(self.panes.get(id), Some(Pane::Editor(v)) if v.dirty))
            .count();
        if unsaved > 0 {
            lines.push(format!(
                "{unsaved} file{} with unsaved changes will be lost.",
                if unsaved == 1 { "" } else { "s" }
            ));
        }
        self.overlay = Some(Overlay::Confirm(Confirm::new(
            format!("Close workspace '{}'?", w.name),
            lines,
            ConfirmAction::CloseWorkspace(w.name.clone()),
            "Close workspace",
        )));
    }

    /// Carry out an accepted confirmation. Targets are looked up again because
    /// the layout may have changed (a shell exiting) while the dialog was open.
    pub fn run_confirmed(&mut self, action: ConfirmAction) {
        match action {
            ConfirmAction::ClosePane(id) => {
                if self.panes.contains_key(&id) {
                    self.close(id);
                }
            }
            ConfirmAction::CloseWorkspace(name) => {
                if self.workspaces.len() > 1 {
                    if let Some(i) = self.workspaces.iter().position(|w| w.name == name) {
                        self.remove_workspace(i);
                    }
                }
            }
        }
    }

    pub fn rename_workspace(&mut self, i: usize, raw: &str) -> Result<(), String> {
        let name = workspace::clean_name(raw).map_err(str::to_string)?;
        let taken = self.workspaces.iter().enumerate().any(|(j, w)| j != i && w.name == name);
        if taken {
            return Err(format!("another workspace is already called '{name}'"));
        }
        match self.workspaces.get_mut(i) {
            Some(w) => {
                w.name = name;
                Ok(())
            }
            None => Err("that workspace no longer exists".to_string()),
        }
    }

    // ---- agents ----------------------------------------------------------

    /// What the agent in `pane` is doing; `None` if the pane has no agent.
    pub fn agent_state(&self, pane: PaneId) -> Option<AgentState> {
        let Some(Pane::Terminal(t)) = self.panes.get(&pane) else { return None };
        t.agent()?;
        Some(match t.status() {
            Status::Blocked => AgentState::Blocked,
            Status::Working => AgentState::Working,
            Status::Idle if self.done.contains(&pane) => AgentState::Done,
            Status::Idle => AgentState::Idle,
        })
    }

    /// Every agent in every workspace, in workspace then layout order.
    pub fn agents(&self) -> Vec<AgentEntry> {
        let mut out = Vec::new();
        for ws in 0..self.workspaces.len() {
            for pane in self.ws_ids(ws) {
                let (Some(state), Some(Pane::Terminal(t))) =
                    (self.agent_state(pane), self.panes.get(&pane))
                else {
                    continue;
                };
                let name = t.agent().unwrap_or("agent").to_string();
                out.push(AgentEntry { pane, workspace: ws, name, state });
            }
        }
        out
    }

    /// The most urgent state among a workspace's agents; `None` if it has none.
    pub fn workspace_state(&self, ws: usize) -> Option<AgentState> {
        AgentState::rollup(self.ws_ids(ws).into_iter().filter_map(|id| self.agent_state(id)))
    }

    /// Called every tick: notice new agents, and mark an agent that finished
    /// out of sight as done until its pane is looked at.
    pub fn track_agents(&mut self) {
        for pane in self.panes.values_mut() {
            if let Pane::Terminal(t) = pane {
                t.probe_agent();
            }
        }
        let current: Vec<(PaneId, Option<Status>)> = self
            .panes
            .iter()
            .filter_map(|(id, p)| match p {
                Pane::Terminal(t) => Some((*id, t.agent().map(|_| t.status()))),
                _ => None,
            })
            .collect();
        for (id, status) in current {
            let Some(status) = status else {
                self.prev_state.remove(&id);
                self.done.remove(&id);
                continue;
            };
            let before = self.prev_state.insert(id, status);
            match status {
                Status::Working => {
                    self.done.remove(&id);
                }
                Status::Idle if before == Some(Status::Working) && id != self.focus => {
                    self.done.insert(id);
                }
                _ => {}
            }
        }
        // Looking at a pane is what clears "done".
        self.done.remove(&self.focus);
    }

    // ---- pane management -------------------------------------------------

    fn open(&mut self, kind: Kind, dir: Dir) {
        self.open_at(kind, dir, false);
    }

    /// Split the focused pane with a new one placed after it, or before it
    /// (left or above) when `before` is set.
    fn open_at(&mut self, kind: Kind, dir: Dir, before: bool) {
        let id = self.next_id;
        let root = self.ws_root();
        let pane = match kind {
            Kind::Terminal => match Terminal::spawn(&root, 24, 80, &pane_env(&self.session, id)) {
                Ok(t) => Pane::Terminal(t),
                Err(e) => {
                    self.notice = Some(format!("failed to start shell: {e}"));
                    return;
                }
            },
            Kind::Graph => Pane::Graph(Box::new(GraphView::new(&root))),
            Kind::Files => match FilesView::new(root.clone()) {
                Ok(v) => Pane::Files(v),
                Err(e) => {
                    self.notice = Some(format!("failed to open files: {e}"));
                    return;
                }
            },
            Kind::Editor(path) => match EditorView::open(&path) {
                Ok(v) => Pane::Editor(v),
                Err(e) => {
                    self.notice = Some(format!("cannot open {}: {e}", path.display()));
                    return;
                }
            },
        };
        self.next_id += 1;
        if self.tree.split(self.focus, id, dir, before) {
            self.panes.insert(id, pane);
            self.focus = id;
            self.zoomed = None;
            self.sel = None;
        }
    }

    /// Show `path` in the workspace's single file viewer. If a viewer is already
    /// open here it is reused for the new file instead of adding another pane,
    /// unless it holds unsaved changes.
    fn open_file(&mut self, path: PathBuf, edit: bool) {
        let existing = self
            .pane_order()
            .into_iter()
            .find(|id| matches!(self.panes.get(id), Some(Pane::Editor(_))));
        match existing {
            Some(id) => {
                let Some(Pane::Editor(current)) = self.panes.get(&id) else { return };
                if current.path != path {
                    if current.dirty {
                        self.notice = Some(format!(
                            "{} has unsaved changes: save it, or close it, before opening another file",
                            current.name()
                        ));
                        self.focus = id;
                        return;
                    }
                    match EditorView::open(&path) {
                        Ok(v) => {
                            self.panes.insert(id, Pane::Editor(v));
                        }
                        Err(e) => {
                            self.notice = Some(format!("cannot open {}: {e}", path.display()));
                            return;
                        }
                    }
                }
                self.focus = id;
                if self.zoomed.is_some() {
                    self.zoomed = Some(id);
                }
            }
            None => self.open(Kind::Editor(path), Dir::Horizontal),
        }
        if let Some(Pane::Editor(v)) = self.panes.get_mut(&self.focus) {
            if edit {
                v.mode = Mode::Edit;
            }
        }
    }

    /// Close a pane, asking first when that would stop a shell or lose unsaved
    /// edits. Browsers and graphs hold nothing, so they close straight away.
    fn request_close(&mut self, id: PaneId) {
        let (title, mut lines) = match self.panes.get(&id) {
            Some(Pane::Terminal(_)) => (
                "Close terminal pane?".to_string(),
                vec!["The shell and anything running in it will be stopped.".to_string()],
            ),
            Some(Pane::Editor(v)) if v.dirty => (
                format!("Close {}?", v.name()),
                vec![format!("Unsaved changes to {} will be lost.", v.name())],
            ),
            Some(_) => return self.close(id),
            None => return,
        };
        // Closing the only pane of a workspace takes the workspace with it.
        if let Some(ws) = self.workspace_of(id) {
            if self.ws_ids(ws).len() <= 1 {
                lines.push(if self.workspaces.len() <= 1 {
                    "This is the last pane, so the session will end.".to_string()
                } else {
                    format!(
                        "It is the last pane, so workspace '{}' will close too.",
                        self.workspaces[ws].name
                    )
                });
            }
        }
        self.overlay = Some(Overlay::Confirm(Confirm::new(
            title,
            lines,
            ConfirmAction::ClosePane(id),
            "Close",
        )));
    }

    /// Pane ids in layout order.
    pub fn pane_order(&self) -> Vec<PaneId> {
        let mut ids = Vec::new();
        self.tree.ids(&mut ids);
        ids
    }

    /// Close a pane wherever it lives. Closing a workspace's last pane closes
    /// the workspace; closing the last pane of the last workspace ends the session.
    fn close(&mut self, id: PaneId) {
        let Some(ws) = self.workspace_of(id) else {
            self.panes.remove(&id);
            return;
        };
        if self.ws_ids(ws).len() <= 1 {
            if self.workspaces.len() <= 1 {
                self.quit = true;
            } else {
                self.remove_workspace(ws);
            }
            return;
        }
        self.panes.remove(&id);
        self.done.remove(&id);
        self.prev_state.remove(&id);
        if ws == self.active {
            self.tree.remove(id);
            if self.zoomed == Some(id) {
                self.zoomed = None;
            }
            if self.sel.as_ref().is_some_and(|s| s.pane == id) {
                self.sel = None;
            }
            if self.focus == id {
                self.focus = self.pane_order().first().copied().unwrap_or(0);
            }
        } else {
            let w = &mut self.workspaces[ws];
            w.tree.remove(id);
            if w.zoomed == Some(id) {
                w.zoomed = None;
            }
            if w.focus == id {
                let mut ids = Vec::new();
                w.tree.ids(&mut ids);
                w.focus = ids.first().copied().unwrap_or(0);
            }
        }
    }

    /// Close panes whose shell has exited.
    pub fn reap(&mut self) {
        let dead: Vec<PaneId> = self
            .panes
            .iter_mut()
            .filter_map(|(id, p)| match p {
                Pane::Terminal(t) => (!t.is_alive()).then_some(*id),
                _ => None,
            })
            .collect();
        for id in dead {
            self.close(id);
        }
    }

    fn toggle_zoom(&mut self) {
        self.zoomed = if self.zoomed.is_some() { None } else { Some(self.focus) };
    }

    fn cycle_focus(&mut self, back: bool) {
        let ids = self.pane_order();
        if let Some(i) = ids.iter().position(|&id| id == self.focus) {
            let n = ids.len();
            self.focus = ids[(i + if back { n - 1 } else { 1 }) % n];
            if self.zoomed.is_some() {
                self.zoomed = Some(self.focus);
            }
            self.sel = None;
        }
    }

    fn focus_dir(&mut self, dx: i32, dy: i32) {
        let Some(&(_, cur)) = self.rects.iter().find(|(id, _)| *id == self.focus) else {
            return;
        };
        let center = |r: Rect| (r.x as i32 * 2 + r.width as i32, r.y as i32 * 2 + r.height as i32);
        let overlap = |a0: u16, a1: u16, b0: u16, b1: u16| a0 < b1 && b0 < a1;
        let (cx, cy) = center(cur);
        let best = self
            .rects
            .iter()
            .filter(|(id, _)| *id != self.focus)
            .filter(|(_, r)| {
                let (x, y) = center(*r);
                if dx != 0 {
                    (x - cx) * dx > 0 && overlap(r.y, r.bottom(), cur.y, cur.bottom())
                } else {
                    (y - cy) * dy > 0 && overlap(r.x, r.right(), cur.x, cur.right())
                }
            })
            .min_by_key(|(_, r)| {
                let (x, y) = center(*r);
                (x - cx).abs() + (y - cy).abs()
            });
        if let Some(&(id, _)) = best {
            self.focus = id;
        }
    }

    fn pane_at(&self, x: u16, y: u16) -> Option<PaneId> {
        self.rects.iter().find(|(_, r)| contains(*r, x, y)).map(|(id, _)| *id)
    }

    fn rect_of(&self, id: PaneId) -> Option<Rect> {
        self.rects.iter().find(|(i, _)| *i == id).map(|(_, r)| *r)
    }

    // ---- input -----------------------------------------------------------

    pub fn on_event(&mut self, ev: Event) -> Result<()> {
        match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => self.on_key(k)?,
            Event::Mouse(m) => self.on_mouse(m)?,
            Event::Paste(_) if self.overlay.is_some() => {}
            Event::Paste(s) => match self.panes.get_mut(&self.focus) {
                Some(Pane::Terminal(t)) => {
                    let bracketed = t.with_screen(|s| s.bracketed_paste());
                    if bracketed {
                        t.write(b"\x1b[200~");
                    }
                    t.write(s.as_bytes());
                    if bracketed {
                        t.write(b"\x1b[201~");
                    }
                }
                Some(Pane::Editor(v)) => v.paste(&s),
                Some(Pane::Files(v)) => v.paste_text(&s),
                _ => {}
            },
            _ => {}
        }
        Ok(())
    }

    fn on_key(&mut self, key: KeyEvent) -> Result<()> {
        self.notice = None;
        self.sel = None;

        if let Some(mut overlay) = self.overlay.take() {
            if overlay.key(self, key) {
                // The dialog may have opened another one (it cannot today), so
                // only restore it if the slot is still empty.
                self.overlay.get_or_insert(overlay);
            }
            return Ok(());
        }

        if let Some(mut menu) = self.menu.take() {
            let n = menu.items.len();
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => {}
                KeyCode::Up | KeyCode::Char('k') => {
                    menu.selected = (menu.selected + n - 1) % n;
                    self.menu = Some(menu);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    menu.selected = (menu.selected + 1) % n;
                    self.menu = Some(menu);
                }
                KeyCode::Enter => self.run_menu(menu.target, menu.items[menu.selected].1),
                _ => self.menu = Some(menu),
            }
            return Ok(());
        }

        let combo = KeyCombo::from_event(key);
        let is_prefix_key = combo == self.config.keymap.prefix;
        let scope = self.focus_scope();

        if self.prefix {
            self.prefix = false;
            if is_prefix_key {
                // Prefix twice sends the key itself to the program in the pane.
                let app_cursor = self.focused_app_cursor();
                self.send_to_focus(&key_bytes(key, app_cursor));
            } else if let Some(action) = self.config.keymap.lookup(true, combo, scope) {
                self.run_action(action);
            }
            return Ok(());
        }
        if is_prefix_key {
            self.prefix = true;
            return Ok(());
        }

        // Direct bindings that work from any pane (split, jump to pane, ...).
        if let Some(action) = self.config.keymap.lookup(false, combo, Scope::Global) {
            self.run_action(action);
            return Ok(());
        }

        match self.panes.get_mut(&self.focus) {
            Some(Pane::Terminal(t)) => {
                let app_cursor = t.with_screen(|s| s.application_cursor());
                t.reset_scroll();
                t.write(&key_bytes(key, app_cursor));
            }
            Some(Pane::Graph(v)) => match v.key(key.code) {
                GraphAct::None => {}
                GraphAct::Msg(m) => self.notice = Some(m),
                GraphAct::Copy(text) => {
                    self.notice = Some(format!("copied commit id {}", &text[..text.len().min(12)]));
                    self.outbox.push(STANDARD.encode(text.as_bytes()));
                }
            },
            Some(Pane::Files(v)) => {
                // While a prompt is open every key is text, never a command.
                let cmd = if v.prompt.is_some() {
                    None
                } else {
                    self.config.keymap.lookup(false, combo, Scope::Files)
                };
                match v.key(key.code, cmd, &mut self.file_clip) {
                    Ok(act) => self.files_act(act),
                    Err(e) => self.notice = Some(e.to_string()),
                }
            }
            Some(Pane::Editor(v)) => {
                let cmd = self
                    .config
                    .keymap
                    .lookup(false, combo, Scope::Editor)
                    .filter(|a| v.accepts(*a));
                match v.key(key, cmd) {
                    Ok(msg) => self.notice = msg,
                    Err(e) => self.notice = Some(e.to_string()),
                }
            }
            None => {}
        }
        Ok(())
    }

    /// The keymap scope of the focused pane.
    pub fn focus_scope(&self) -> Scope {
        match self.panes.get(&self.focus) {
            Some(Pane::Files(_)) => Scope::Files,
            Some(Pane::Editor(_)) => Scope::Editor,
            _ => Scope::Global,
        }
    }

    fn focused_app_cursor(&self) -> bool {
        match self.panes.get(&self.focus) {
            Some(Pane::Terminal(t)) => t.with_screen(|s| s.application_cursor()),
            _ => false,
        }
    }

    fn files_act(&mut self, act: FilesAct) {
        match act {
            FilesAct::None => {}
            FilesAct::Msg(m) => self.notice = Some(m),
            FilesAct::Open { path, edit } => self.open_file(path, edit),
        }
    }

    fn send_to_focus(&mut self, bytes: &[u8]) {
        if let Some(Pane::Terminal(t)) = self.panes.get(&self.focus) {
            t.write(bytes);
        }
    }

    fn run_action(&mut self, action: Action) {
        match action {
            Action::SplitLeft => self.open_at(Kind::Terminal, Dir::Horizontal, true),
            Action::SplitRight | Action::NewTerminal => self.open(Kind::Terminal, Dir::Horizontal),
            Action::SplitDown => self.open(Kind::Terminal, Dir::Vertical),
            Action::OpenGraph => self.open(Kind::Graph, Dir::Horizontal),
            Action::OpenFiles => self.open(Kind::Files, Dir::Horizontal),
            Action::ClosePane => self.request_close(self.focus),
            Action::ZoomPane => self.toggle_zoom(),
            Action::NextPane => self.cycle_focus(false),
            Action::PrevPane => self.cycle_focus(true),
            Action::FocusLeft => self.focus_dir(-1, 0),
            Action::FocusRight => self.focus_dir(1, 0),
            Action::FocusUp => self.focus_dir(0, -1),
            Action::FocusDown => self.focus_dir(0, 1),
            Action::ResizeLeft => self.tree.nudge(self.focus, Dir::Horizontal, -0.05),
            Action::ResizeRight => self.tree.nudge(self.focus, Dir::Horizontal, 0.05),
            Action::ResizeUp => self.tree.nudge(self.focus, Dir::Vertical, -0.05),
            Action::ResizeDown => self.tree.nudge(self.focus, Dir::Vertical, 0.05),
            Action::Detach => self.detach = true,
            Action::QuitSession => self.quit = true,
            Action::OpenMenu => self.open_app_menu(),
            Action::OpenKeybinds => self.overlay = Some(Overlay::Keybinds(Keybinds::new())),
            Action::OpenSettings => {
                self.overlay =
                    Some(Overlay::Settings(SettingsDialog::new(self.settings_snapshot())));
            }
            Action::OpenThemes => {
                self.overlay = Some(Overlay::Themes(ThemePicker::new(&self.config.theme)));
            }
            Action::NewWorkspace => self.new_workspace(),
            Action::CloseWorkspace => self.request_close_workspace(self.active),
            Action::RenameWorkspace => {
                let name = self.workspaces[self.active].name.clone();
                self.overlay = Some(Overlay::Name(NameInput::new(self.active, &name)));
            }
            Action::NextWorkspace => self.cycle_workspace(false),
            Action::PrevWorkspace => self.cycle_workspace(true),
            Action::GotoWorkspace(n) => self.goto_workspace(n),
            Action::GotoPane(n) => self.goto_pane(n),
            // File browser and editor actions are resolved against the focused pane.
            _ => {}
        }
    }

    /// The dropdown of the dock's menu button, anchored at that button.
    fn open_app_menu(&mut self) {
        let (x, y) = self.dock.iter().find(|(_, hit)| *hit == DockHit::Menu).map_or(
            (self.area.right().saturating_sub(1), self.area.bottom().saturating_sub(2)),
            |(r, _)| (r.x, r.y),
        );
        self.menu = Some(Menu::app(x, y, self.focus));
    }

    pub fn settings_snapshot(&self) -> settings::Snapshot {
        settings::gather(
            &self.session,
            &self.config.theme,
            self.config.keymap.overrides().len(),
            &self.config.warnings,
        )
    }

    /// Preview or apply a theme without saving it.
    pub fn set_theme(&mut self, name: &str) {
        self.config.theme = name.to_string();
        let (theme, _) = theme::resolve(name, &self.config.colors);
        self.theme = theme;
    }

    /// Persist theme and bindings; returns a short message for the UI.
    pub fn save_config(&mut self) -> String {
        match self.config.save() {
            Ok(()) => {
                // Our own write must not look like an outside edit.
                self.config_mtime = config::modified();
                "saved".to_string()
            }
            Err(e) => format!("could not save config: {e:#}"),
        }
    }

    /// Let an open dialog collect background results (the usage scan).
    pub fn poll_overlay(&mut self) {
        if let Some(overlay) = &mut self.overlay {
            overlay.poll();
        }
    }

    /// Keep open file viewers in step with the disk, so a file an agent just
    /// changed is not shown (or saved over) in its old state.
    pub fn poll_files(&mut self) {
        for pane in self.panes.values_mut() {
            if let Pane::Editor(v) = pane {
                v.refresh_if_clean();
            }
        }
    }

    /// Pick up edits made to the config file while the session is running.
    pub fn poll_config(&mut self) {
        if self.config_checked.elapsed() < CONFIG_POLL {
            return;
        }
        self.config_checked = Instant::now();
        let modified = config::modified();
        if modified == self.config_mtime || self.overlay.is_some() {
            return;
        }
        self.config_mtime = modified;
        self.config = Config::load();
        let (theme, warnings) = theme::resolve(&self.config.theme, &self.config.colors);
        self.theme = theme;
        self.config.warnings.extend(warnings);
        self.notice = Some(match self.config.warnings.first() {
            Some(w) => format!("config reloaded with warnings: {w}"),
            None => "config reloaded".to_string(),
        });
    }

    /// The folder a pane is about: the browser's directory, the open file's
    /// folder, otherwise the workspace root.
    fn pane_folder(&self, id: PaneId) -> PathBuf {
        match self.panes.get(&id) {
            Some(Pane::Files(v)) => v.fb.dir.clone(),
            Some(Pane::Editor(v)) => {
                v.path.parent().map_or_else(|| self.ws_root(), std::path::Path::to_path_buf)
            }
            _ => self.ws_root(),
        }
    }

    fn run_menu(&mut self, target: PaneId, action: MenuAction) {
        // The workspace menu's target is a workspace index, not a pane.
        match action {
            MenuAction::NewWorkspace => return self.new_workspace(),
            MenuAction::RenameWorkspace => {
                if let Some(w) = self.workspaces.get(target) {
                    self.overlay = Some(Overlay::Name(NameInput::new(target, &w.name)));
                }
                return;
            }
            MenuAction::CloseWorkspace => return self.request_close_workspace(target),
            _ => {}
        }
        self.focus_pane(target);
        match action {
            MenuAction::SplitRight => self.open(Kind::Terminal, Dir::Horizontal),
            MenuAction::SplitDown => self.open(Kind::Terminal, Dir::Vertical),
            MenuAction::NewGraph => self.open(Kind::Graph, Dir::Horizontal),
            MenuAction::NewFiles => self.open(Kind::Files, Dir::Horizontal),
            MenuAction::Zoom => self.toggle_zoom(),
            MenuAction::OpenExplorer => {
                let dir = self.pane_folder(target);
                self.notice = Some(match crate::file_browser::open_in_file_manager(&dir) {
                    Ok(()) => format!("opened {}", dir.display()),
                    Err(e) => format!("cannot open {}: {e}", dir.display()),
                });
            }
            MenuAction::Close => self.request_close(target),
            MenuAction::Keybindings => self.run_action(Action::OpenKeybinds),
            MenuAction::Settings => self.run_action(Action::OpenSettings),
            MenuAction::Themes => self.run_action(Action::OpenThemes),
            MenuAction::Detach => self.detach = true,
            MenuAction::QuitSession => self.quit = true,
            MenuAction::NewWorkspace | MenuAction::RenameWorkspace | MenuAction::CloseWorkspace => {
            }
        }
    }

    /// Hand a mouse event to the program in the pane when it asked for mouse
    /// reporting. Returns true if the event was consumed. Holding Shift, or
    /// clicking a pane border, keeps the event for yurlungr itself.
    fn forward_mouse(&mut self, id: PaneId, m: &MouseEvent) -> bool {
        let Some(rect) = self.rect_of(id) else { return false };
        let area = inner(rect);
        if !contains(area, m.column, m.row) {
            return false;
        }
        let Some(Pane::Terminal(t)) = self.panes.get(&id) else { return false };
        let (mode, enc) = t.mouse_mode();
        if mode == vt100::MouseProtocolMode::None {
            return false;
        }
        if let Some(bytes) =
            mouse::encode(m.kind, m.modifiers, m.column - area.x, m.row - area.y, mode, enc)
        {
            t.write(&bytes);
        }
        if matches!(m.kind, MouseEventKind::Down(_)) {
            self.focus = id;
        }
        true
    }

    fn on_mouse(&mut self, m: MouseEvent) -> Result<()> {
        let (x, y) = (m.column, m.row);

        // A dialog owns the screen: the wheel scrolls it, a click can press its
        // buttons, everything else is ignored.
        if let Some(mut overlay) = self.overlay.take() {
            let keep = match m.kind {
                MouseEventKind::ScrollUp => {
                    overlay.scroll(-3);
                    true
                }
                MouseEventKind::ScrollDown => {
                    overlay.scroll(3);
                    true
                }
                MouseEventKind::Down(MouseButton::Left) => overlay.click(self, x, y),
                _ => true,
            };
            if keep {
                self.overlay.get_or_insert(overlay);
            }
            return Ok(());
        }

        let free = self.drag.is_none() && self.menu.is_none() && self.selecting.is_none();
        if free && !m.modifiers.contains(KeyModifiers::SHIFT) {
            if let Some(id) = self.pane_at(x, y) {
                if self.forward_mouse(id, &m) {
                    return Ok(());
                }
            }
        }

        match m.kind {
            MouseEventKind::Moved => {
                let screen = self.area;
                if let Some(menu) = &mut self.menu {
                    if let Some(i) = menu.item_at(x, y, screen) {
                        menu.selected = i;
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Right) => {
                self.menu = None;
                if let Some(hit) = self.dock_at(x, y) {
                    match hit {
                        DockHit::Workspace(i) => self.menu = Some(Menu::workspace(x, y, i)),
                        DockHit::Agent(id) => {
                            self.focus_pane(id);
                            self.menu = Some(Menu::new(x, y, id));
                        }
                        DockHit::NewWorkspace | DockHit::Menu => {}
                    }
                } else if let Some(id) = self.pane_at(x, y) {
                    self.focus = id;
                    self.menu = Some(Menu::new(x, y, id));
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.sel = None;
                if let Some(menu) = self.menu.take() {
                    if let Some(i) = menu.item_at(x, y, self.area) {
                        self.run_menu(menu.target, menu.items[i].1);
                    }
                    return Ok(());
                }
                if let Some(hit) = self.dock_at(x, y) {
                    match hit {
                        DockHit::Workspace(i) => self.switch_workspace(i),
                        DockHit::Agent(id) => self.focus_pane(id),
                        DockHit::NewWorkspace => self.new_workspace(),
                        DockHit::Menu => self.open_app_menu(),
                    }
                    return Ok(());
                }
                if let Some(d) = self.dividers.iter().find(|d| contains(d.rect, x, y)) {
                    self.drag = Some(Drag { path: d.path.clone(), dir: d.dir, area: d.area });
                    return Ok(());
                }
                if let Some(id) = self.pane_at(x, y) {
                    self.focus = id;
                    self.click_pane(id, x, y)?;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(id) = self.selecting {
                    if let Some(rect) = self.rect_of(id) {
                        let a = inner(rect);
                        let col = x.clamp(a.x, a.right().saturating_sub(1)) - a.x;
                        let row = y.clamp(a.y, a.bottom().saturating_sub(1)) - a.y;
                        if let Some(sel) = &mut self.sel {
                            sel.end = (row, col);
                        }
                    }
                } else if let Some(d) = &self.drag {
                    let ratio = match d.dir {
                        Dir::Horizontal => {
                            x.saturating_sub(d.area.x) as f32 / d.area.width.max(1) as f32
                        }
                        Dir::Vertical => {
                            y.saturating_sub(d.area.y) as f32 / d.area.height.max(1) as f32
                        }
                    };
                    self.tree.set_ratio(&d.path, ratio);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.drag = None;
                if self.selecting.take().is_some() {
                    if self.sel.as_ref().is_some_and(|s| s.start != s.end) {
                        self.copy_selection();
                    } else {
                        self.sel = None;
                    }
                }
            }
            MouseEventKind::ScrollUp => self.scroll_at(x, y, 1),
            MouseEventKind::ScrollDown => self.scroll_at(x, y, -1),
            _ => {}
        }
        Ok(())
    }

    /// `dir` is positive for scrolling up (towards older content).
    fn scroll_at(&mut self, x: u16, y: u16, dir: isize) {
        let Some(id) = self.pane_at(x, y) else { return };
        match self.panes.get_mut(&id) {
            Some(Pane::Terminal(t)) => {
                self.sel = None;
                t.scroll(dir * 3);
            }
            Some(Pane::Graph(v)) => {
                if contains(v.detail_area, x, y) {
                    v.scroll_detail(-dir * 3);
                } else {
                    v.scroll(-dir);
                }
            }
            Some(Pane::Files(v)) => v.scroll(-dir),
            Some(Pane::Editor(v)) => v.scroll_view(-dir * 3),
            None => {}
        }
    }

    fn dock_at(&self, x: u16, y: u16) -> Option<DockHit> {
        self.dock.iter().find(|(r, _)| contains(*r, x, y)).map(|(_, hit)| *hit)
    }

    fn click_pane(&mut self, id: PaneId, x: u16, y: u16) -> Result<()> {
        let Some(rect) = self.rect_of(id) else { return Ok(()) };
        let area = inner(rect);
        match self.panes.get_mut(&id) {
            Some(Pane::Terminal(_)) => {
                if contains(area, x, y) {
                    let pos = (y - area.y, x - area.x);
                    self.sel = Some(Selection { pane: id, start: pos, end: pos });
                    self.selecting = Some(id);
                }
            }
            Some(Pane::Graph(v)) => {
                // Only the commit list is clickable; the detail panel below it is not.
                if contains(v.list_area, x, y) {
                    let idx = v.state.offset() + (y - v.list_area.y) as usize;
                    v.select(idx);
                }
            }
            Some(Pane::Files(v)) => {
                if let Some(row) = y.checked_sub(area.y) {
                    let idx = v.state.offset() + row as usize;
                    let act = v.click(idx)?;
                    self.files_act(act);
                }
            }
            Some(Pane::Editor(v)) => {
                let text_x = area.x + v.gutter() as u16;
                if contains(area, x, y) && x >= text_x {
                    v.click((y - area.y) as usize, (x - text_x) as usize);
                }
            }
            None => {}
        }
        Ok(())
    }

    /// Queue the selected text as an OSC 52 clipboard write for the client.
    fn copy_selection(&mut self) {
        let Some(sel) = &self.sel else { return };
        let ((r0, c0), (r1, c1)) = sel.ordered();
        let Some(Pane::Terminal(t)) = self.panes.get(&sel.pane) else { return };
        let text = t.with_screen(|s| {
            let cols = s.size().1;
            s.contents_between(r0, c0, r1, (c1 + 1).min(cols))
        });
        let text = text.lines().map(str::trim_end).collect::<Vec<_>>().join("\n");
        if text.is_empty() {
            return;
        }
        self.outbox.push(STANDARD.encode(text.as_bytes()));
        self.notice = Some(format!("copied {} characters", text.chars().count()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{keymap::Action as KeyAction, proto::ReportState};

    fn viewers(app: &App) -> Vec<&EditorView> {
        app.panes
            .values()
            .filter_map(|p| if let Pane::Editor(v) = p { Some(v) } else { None })
            .collect()
    }

    fn setup() -> (tempfile::TempDir, App, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.txt"), dir.path().join("b.txt"));
        fs::write(&a, "alpha\n").unwrap();
        fs::write(&b, "beta\n").unwrap();
        let app = App::blank(dir.path().to_path_buf(), None, "test");
        (dir, app, a, b)
    }

    #[test]
    fn opening_another_file_reuses_the_single_viewer() {
        let (_d, mut app, a, b) = setup();
        app.open_file(a.clone(), false);
        assert_eq!(viewers(&app).len(), 1);
        assert_eq!(app.panes.len(), 1);

        app.open_file(b.clone(), false);
        assert_eq!(viewers(&app).len(), 1, "still one viewer");
        assert_eq!(viewers(&app)[0].path, b, "now showing the new file");
        assert_eq!(app.panes.len(), 1, "no extra pane was created");

        app.open_file(b.clone(), true);
        assert_eq!(viewers(&app).len(), 1, "reopening the same file is idempotent");
        assert!(viewers(&app)[0].mode == Mode::Edit, "but can switch it to edit mode");
    }

    #[test]
    fn a_viewer_with_unsaved_changes_is_never_replaced() {
        let (_d, mut app, a, b) = setup();
        app.open_file(a.clone(), true);
        let id = app.focus;
        if let Some(Pane::Editor(v)) = app.panes.get_mut(&id) {
            v.paste("typed ");
            assert!(v.dirty);
        }
        app.open_file(b, false);
        assert_eq!(viewers(&app)[0].path, a, "kept the file being edited");
        assert!(viewers(&app)[0].dirty);
        assert!(app.notice.as_deref().is_some_and(|n| n.contains("unsaved")), "{:?}", app.notice);
    }

    #[test]
    fn unreadable_file_leaves_the_viewer_alone() {
        let (d, mut app, a, _b) = setup();
        app.open_file(a.clone(), false);
        let bin = d.path().join("bin");
        fs::write(&bin, [0u8, 1, 2]).unwrap();
        app.open_file(bin, false);
        assert_eq!(viewers(&app)[0].path, a);
        assert!(app.notice.as_deref().is_some_and(|n| n.contains("binary")), "{:?}", app.notice);
    }

    /// An app whose first pane is a git graph, so no shell is needed.
    fn light_app() -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::blank(dir.path().to_path_buf(), None, "test");
        app.panes.insert(0, Pane::Graph(Box::new(GraphView::new(dir.path()))));
        (dir, app)
    }

    #[test]
    fn workspaces_keep_independent_layouts() {
        let (_d, mut app) = light_app();
        app.new_workspace();
        assert_eq!(app.workspaces.len(), 2);
        assert_eq!(app.active, 1);
        assert_eq!(app.pane_order().len(), 1, "a new workspace starts with one pane");
        let second_first = app.pane_order()[0];

        app.run_action(KeyAction::SplitRight);
        assert_eq!(app.pane_order().len(), 2);

        app.switch_workspace(0);
        assert_eq!(app.pane_order(), [0], "the first workspace is untouched");
        assert_eq!(app.ws_ids(1).len(), 2, "the other one kept its split");

        app.switch_workspace(1);
        assert_eq!(app.pane_order().len(), 2);
        assert_eq!(app.pane_order()[0], second_first);
        assert_eq!(app.panes.len(), 3, "all panes live in one map with unique ids");
    }

    #[test]
    fn closing_panes_and_workspaces() {
        let (_d, mut app) = light_app();
        app.new_workspace();
        app.run_action(KeyAction::SplitRight);
        let ids = app.pane_order();
        app.switch_workspace(0);

        // A pane in a workspace that is not showing.
        app.close(ids[1]);
        assert_eq!(app.ws_ids(1), [ids[0]]);
        assert!(!app.panes.contains_key(&ids[1]));

        // Its last pane takes the whole workspace with it.
        app.close(ids[0]);
        assert_eq!(app.workspaces.len(), 1);
        assert_eq!(app.active, 0);
        assert!(!app.quit);

        // The very last pane ends the session.
        app.close(0);
        assert!(app.quit);
    }

    /// Answer the open confirmation dialog with a key, as a user would.
    fn answer(app: &mut App, code: KeyCode) {
        let mut dialog = app.overlay.take().expect("a dialog is open");
        let keep = dialog.key(app, KeyEvent::new(code, KeyModifiers::NONE));
        if keep {
            app.overlay = Some(dialog);
        }
    }

    fn confirm_text(app: &App) -> String {
        match &app.overlay {
            Some(Overlay::Confirm(c)) => format!("{} {}", c.title(), c.text()),
            _ => panic!("no confirmation dialog is open"),
        }
    }

    #[test]
    fn closing_the_active_workspace_asks_first() {
        let (_d, mut app) = light_app();
        app.new_workspace();
        app.new_workspace();
        assert_eq!((app.active, app.workspaces.len()), (2, 3));

        app.request_close_workspace(2);
        assert_eq!(app.workspaces.len(), 3, "nothing happens until answered");
        assert!(confirm_text(&app).contains("1 pane"), "{}", confirm_text(&app));
        answer(&mut app, KeyCode::Enter);
        assert_eq!(app.workspaces.len(), 3, "Enter alone keeps it: No is the default");
        assert!(app.overlay.is_none());

        app.request_close_workspace(2);
        answer(&mut app, KeyCode::Esc);
        assert_eq!(app.workspaces.len(), 3, "Esc keeps it");

        app.request_close_workspace(2);
        answer(&mut app, KeyCode::Char('y'));
        assert_eq!(
            (app.active, app.workspaces.len()),
            (1, 2),
            "y closes it and moves to a neighbour"
        );
        assert_eq!(app.panes.len(), 2, "its shell is gone");

        app.request_close_workspace(0);
        answer(&mut app, KeyCode::Right); // choose Yes
        answer(&mut app, KeyCode::Enter);
        assert_eq!(
            (app.active, app.workspaces.len()),
            (0, 1),
            "indexes shift when an earlier one closes"
        );

        app.request_close_workspace(0);
        assert!(app.overlay.is_none(), "no dialog for the only workspace");
        assert!(
            app.notice.as_deref().is_some_and(|n| n.contains("only workspace")),
            "{:?}",
            app.notice
        );
    }

    #[test]
    fn a_confirmed_workspace_close_survives_the_layout_changing() {
        let (_d, mut app) = light_app();
        app.new_workspace();
        app.new_workspace();
        app.rename_workspace(2, "last").unwrap();
        app.request_close_workspace(2);
        // While the dialog is open an earlier workspace goes away (its shell exited).
        app.remove_workspace(0);
        assert_eq!(app.workspaces.iter().position(|w| w.name == "last"), Some(1));
        answer(&mut app, KeyCode::Char('y'));
        let names: Vec<_> = app.workspaces.iter().map(|w| w.name.as_str()).collect();
        assert!(!names.contains(&"last"), "closed the workspace that was asked about: {names:?}");
        assert_eq!(names.len(), 1);
    }

    #[test]
    fn closing_a_terminal_pane_asks_first() {
        let (_d, mut app) = light_app();
        app.new_workspace();
        app.run_action(KeyAction::SplitRight);
        let target = app.pane_order()[1];
        assert!(matches!(app.panes[&target], Pane::Terminal(_)));

        app.run_action(KeyAction::ClosePane);
        assert!(confirm_text(&app).contains("Close terminal pane"), "{}", confirm_text(&app));
        assert!(!confirm_text(&app).contains("last pane"), "two panes here");
        answer(&mut app, KeyCode::Char('n'));
        assert!(app.panes.contains_key(&target), "declined");

        app.run_action(KeyAction::ClosePane);
        answer(&mut app, KeyCode::Char('y'));
        assert!(!app.panes.contains_key(&target), "confirmed");
        assert_eq!(app.pane_order().len(), 1);
    }

    #[test]
    fn the_prompt_says_when_closing_ends_a_workspace_or_the_session() {
        let (_d, mut app) = light_app();
        app.new_workspace();
        app.run_action(KeyAction::ClosePane); // the only pane of workspace 2
        assert!(confirm_text(&app).contains("workspace"), "{}", confirm_text(&app));
        answer(&mut app, KeyCode::Char('y'));
        assert_eq!(app.workspaces.len(), 1, "the workspace closed with its last pane");

        // A shell as the very last pane of the session.
        let id = app.next_id;
        app.next_id += 1;
        let shell = Terminal::spawn(&app.ws_root(), 24, 80, &pane_env("t", id)).unwrap();
        app.panes.clear();
        app.panes.insert(id, Pane::Terminal(shell));
        app.tree = Node::Leaf(id);
        app.focus = id;
        app.request_close(id);
        assert!(confirm_text(&app).contains("session will end"), "{}", confirm_text(&app));
        answer(&mut app, KeyCode::Char('y'));
        assert!(app.quit);
    }

    #[test]
    fn panes_that_hold_nothing_close_without_a_prompt() {
        let (_d, mut app) = light_app();
        app.run_action(KeyAction::OpenFiles);
        let files = app.focus;
        app.run_action(KeyAction::ClosePane);
        assert!(app.overlay.is_none());
        assert!(!app.panes.contains_key(&files));
        assert!(app.panes.contains_key(&0), "the graph pane remains");
    }

    #[test]
    fn unsaved_editors_prompt_and_clean_ones_do_not() {
        let (_d, mut app, file, _other) = setup();
        // A graph pane first, so the viewer is never the workspace's last pane.
        let dir = file.parent().unwrap().to_path_buf();
        app.panes.insert(0, Pane::Graph(Box::new(GraphView::new(&dir))));

        app.open_file(file.clone(), false);
        let viewer = app.focus;
        app.run_action(KeyAction::ClosePane);
        assert!(app.overlay.is_none(), "a clean viewer closes at once");
        assert!(!app.panes.contains_key(&viewer));

        app.open_file(file, true);
        let editor = app.focus;
        if let Some(Pane::Editor(v)) = app.panes.get_mut(&editor) {
            v.paste("changed ");
        }
        app.run_action(KeyAction::ClosePane);
        assert!(confirm_text(&app).contains("Unsaved changes"), "{}", confirm_text(&app));
        answer(&mut app, KeyCode::Char('n'));
        assert!(app.panes.contains_key(&editor), "declined");
        app.run_action(KeyAction::ClosePane);
        answer(&mut app, KeyCode::Char('y'));
        assert!(!app.panes.contains_key(&editor), "confirmed");
    }

    #[test]
    fn rename_validates_and_rejects_duplicates() {
        let (_d, mut app) = light_app();
        app.new_workspace();
        app.rename_workspace(0, "  api  ").unwrap();
        assert_eq!(app.workspaces[0].name, "api");
        assert!(app.rename_workspace(1, "api").unwrap_err().contains("already"));
        assert!(app.rename_workspace(1, "   ").is_err());
        assert!(app.rename_workspace(1, "api").is_err());
        app.rename_workspace(0, "api").unwrap(); // keeping its own name is fine
        assert!(!app.workspaces[1].name.is_empty());
    }

    #[test]
    fn new_workspaces_get_unique_default_names() {
        let (_d, mut app) = light_app();
        let base = app.workspaces[0].name.clone();
        app.new_workspace();
        app.new_workspace();
        assert_eq!(app.workspaces[1].name, format!("{base} 2"));
        assert_eq!(app.workspaces[2].name, format!("{base} 3"));
    }

    #[test]
    fn jumping_between_workspaces() {
        let (_d, mut app) = light_app();
        app.new_workspace();
        app.new_workspace();
        app.run_action(KeyAction::GotoWorkspace(1));
        assert_eq!(app.active, 0);
        app.run_action(KeyAction::NextWorkspace);
        assert_eq!(app.active, 1);
        app.run_action(KeyAction::PrevWorkspace);
        app.run_action(KeyAction::PrevWorkspace);
        assert_eq!(app.active, 2, "previous wraps around");
        app.run_action(KeyAction::GotoWorkspace(9));
        assert_eq!(app.active, 2, "out of range keeps the workspace");
        assert!(app.notice.is_some());
    }

    #[test]
    fn goto_pane_stays_inside_the_current_workspace() {
        let (_d, mut app) = light_app();
        app.new_workspace();
        app.run_action(KeyAction::SplitRight);
        app.run_action(KeyAction::SplitDown); // three panes in workspace 2
        let order = app.pane_order();
        assert_eq!(order.len(), 3);

        app.run_action(KeyAction::GotoPane(1));
        assert_eq!(app.focus, order[0]);
        app.run_action(KeyAction::GotoPane(3));
        assert_eq!(app.focus, order[2]);
        assert_eq!(app.active, 1, "never leaves the workspace");

        app.run_action(KeyAction::GotoPane(4));
        assert_eq!(app.focus, order[2], "out of range keeps focus");
        assert!(app.notice.as_deref().is_some_and(|n| n.contains("no pane 4")), "{:?}", app.notice);

        // The same digit means a workspace when it comes with ctrl.
        app.run_action(KeyAction::GotoWorkspace(1));
        assert_eq!(app.active, 0);
    }

    #[test]
    fn focusing_a_pane_in_another_workspace_switches_to_it() {
        let (_d, mut app) = light_app();
        app.new_workspace();
        let there = app.pane_order()[0];
        app.switch_workspace(0);
        app.focus_pane(there);
        assert_eq!((app.active, app.focus), (1, there));
    }

    fn set_agent(app: &mut App, pane: PaneId, state: ReportState) {
        if let Some(Pane::Terminal(t)) = app.panes.get_mut(&pane) {
            t.set_report("claude".to_string(), state);
        }
    }

    #[test]
    fn agents_are_listed_across_workspaces_with_rolled_up_state() {
        let (_d, mut app) = light_app();
        app.new_workspace();
        let a = app.pane_order()[0];
        app.run_action(KeyAction::SplitRight);
        let b = app.pane_order()[1];
        assert!(app.agents().is_empty(), "plain shells are not agents");
        assert_eq!(app.workspace_state(1), None);

        set_agent(&mut app, a, ReportState::Idle);
        set_agent(&mut app, b, ReportState::Blocked);
        app.switch_workspace(0);

        let agents = app.agents();
        assert_eq!(agents.len(), 2, "agents in other workspaces are listed too");
        assert!(agents.iter().all(|e| e.workspace == 1 && e.name == "claude"));
        assert_eq!(app.agent_state(a), Some(AgentState::Idle));
        assert_eq!(app.agent_state(b), Some(AgentState::Blocked));
        assert_eq!(app.workspace_state(1), Some(AgentState::Blocked), "most urgent wins");
        assert_eq!(app.workspace_state(0), None, "no agents here");
    }

    #[test]
    fn finishing_out_of_sight_is_done_until_looked_at() {
        let (_d, mut app) = light_app();
        app.new_workspace();
        let pane = app.pane_order()[0];
        set_agent(&mut app, pane, ReportState::Working);
        app.switch_workspace(0); // look elsewhere
        app.track_agents();
        assert_eq!(app.agent_state(pane), Some(AgentState::Working));

        set_agent(&mut app, pane, ReportState::Idle);
        app.track_agents();
        assert_eq!(app.agent_state(pane), Some(AgentState::Done));
        app.track_agents();
        assert_eq!(app.agent_state(pane), Some(AgentState::Done), "stays done while unseen");

        app.focus_pane(pane);
        app.track_agents();
        assert_eq!(app.agent_state(pane), Some(AgentState::Idle), "seen");
    }

    #[test]
    fn finishing_in_sight_is_just_idle() {
        let (_d, mut app) = light_app();
        app.new_workspace();
        let pane = app.pane_order()[0];
        set_agent(&mut app, pane, ReportState::Working);
        app.track_agents();
        set_agent(&mut app, pane, ReportState::Idle);
        app.track_agents();
        assert_eq!(app.agent_state(pane), Some(AgentState::Idle));
    }

    #[test]
    fn layouts_round_trip_through_disk_with_workspaces() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("t.layout.json");
        let mut app = App::blank(dir.path().to_path_buf(), Some(file.clone()), "t");
        // Two workspaces of graph panes (no shells needed).
        app.panes.insert(0, Pane::Graph(Box::new(GraphView::new(dir.path()))));
        app.panes.insert(1, Pane::Graph(Box::new(GraphView::new(dir.path()))));
        app.next_id = 2;
        app.workspaces[0].name = "web".into();
        app.workspaces.push(Workspace {
            name: "api".into(),
            root: dir.path().to_path_buf(),
            tree: Node::Leaf(1),
            focus: 1,
            zoomed: None,
        });
        app.active = 1;
        app.load_active();
        app.persist();

        let back = App::restore(&file, "t").expect("restores");
        let names: Vec<_> = back.workspaces.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(names, ["web", "api"]);
        assert_eq!(back.active, 1);
        assert_eq!(back.pane_order(), [1]);
        assert_eq!(back.panes.len(), 2);
        assert_eq!(back.next_id, 2);
    }

    #[test]
    fn layouts_saved_before_workspaces_still_load() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("old.layout.json");
        let root = dir.path().display().to_string().replace('\\', "/");
        let old = format!(
            r#"{{"root":"{root}","tree":{{"Split":{{"dir":"Horizontal","ratio":0.5,"a":{{"Leaf":0}},"b":{{"Leaf":3}}}}}},"kinds":[[0,"Graph"],[3,"Files"]],"next_id":4,"focus":3}}"#
        );
        fs::write(&file, old).unwrap();
        let app = App::restore(&file, "t").expect("old layouts load");
        assert_eq!(app.workspaces.len(), 1);
        assert_eq!(app.pane_order(), [0, 3]);
        assert_eq!(app.focus, 3);
        assert_eq!(app.next_id, 4);
    }
}
