use std::{
    collections::HashMap,
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
    overlay::{Keybinds, Overlay, SettingsDialog, ThemePicker},
    proto::ReportState,
    settings,
    terminal::{Status, Terminal},
    theme::{self, Theme},
    views::{Clip, FilesAct, FilesView, GraphView, Pane},
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

/// A clickable region of the dock.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DockHit {
    Pane(PaneId),
    New,
    Menu,
}

/// What is written to disk so a new server can rebuild the layout. Processes
/// do not survive a server restart; each terminal pane gets a fresh shell.
#[derive(Serialize, Deserialize)]
struct Saved {
    root: PathBuf,
    tree: Node,
    kinds: Vec<(PaneId, Kind)>,
    next_id: PaneId,
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
    tree: Node,
    next_id: PaneId,
    pub focus: PaneId,
    pub zoomed: Option<PaneId>,
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
    /// An editor with unsaved changes whose close was refused once already.
    pending_close: Option<PaneId>,
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
            root,
            session: session.to_string(),
            panes: HashMap::new(),
            tree: Node::Leaf(0),
            next_id: 1,
            focus: 0,
            zoomed: None,
            prefix: false,
            menu: None,
            notice: None,
            rects: Vec::new(),
            dock: Vec::new(),
            file_clip: None,
            pending_close: None,
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
        app.tree = saved.tree;
        app.next_id = saved.next_id;
        app.focus = saved.focus;

        let mut tree_ids = Vec::new();
        app.tree.ids(&mut tree_ids);
        if tree_ids.iter().any(|id| !saved.kinds.iter().any(|(k, _)| k == id)) {
            return None;
        }
        for (id, kind) in saved.kinds {
            let pane = match kind {
                Kind::Terminal => Terminal::spawn(&saved.root, 24, 80, &pane_env(session, id))
                    .ok()
                    .map(Pane::Terminal),
                Kind::Graph => Some(Pane::Graph(GraphView::new(&saved.root))),
                Kind::Files => FilesView::new(saved.root.clone()).ok().map(Pane::Files),
                Kind::Editor(path) => EditorView::open(&path).ok().map(Pane::Editor),
            };
            match pane {
                Some(p) => {
                    app.panes.insert(id, p);
                }
                None => {
                    if tree_ids.len() > 1 {
                        app.tree.remove(id);
                    }
                }
            }
        }
        if app.panes.is_empty() {
            return None;
        }
        if !app.panes.contains_key(&app.focus) {
            app.focus = *app.panes.keys().next()?;
        }
        Some(app)
    }

    /// Write the layout to disk when it changed.
    pub fn persist(&mut self) {
        let Some(path) = &self.layout_file else { return };
        let mut kinds: Vec<(PaneId, Kind)> =
            self.panes.iter().map(|(id, p)| (*id, kind_of(p))).collect();
        kinds.sort_by_key(|(id, _)| *id);
        let saved = Saved {
            root: self.root.clone(),
            tree: self.tree.clone(),
            kinds,
            next_id: self.next_id,
            focus: self.focus,
        };
        if let Ok(json) = serde_json::to_string(&saved) {
            if json != self.last_saved && fs::write(path, &json).is_ok() {
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

    // ---- pane management -------------------------------------------------

    fn open(&mut self, kind: Kind, dir: Dir) {
        self.open_at(kind, dir, false);
    }

    /// Split the focused pane with a new one placed after it, or before it
    /// (left or above) when `before` is set.
    fn open_at(&mut self, kind: Kind, dir: Dir, before: bool) {
        let id = self.next_id;
        let pane = match kind {
            Kind::Terminal => {
                match Terminal::spawn(&self.root, 24, 80, &pane_env(&self.session, id)) {
                    Ok(t) => Pane::Terminal(t),
                    Err(e) => {
                        self.notice = Some(format!("failed to start shell: {e}"));
                        return;
                    }
                }
            }
            Kind::Graph => Pane::Graph(GraphView::new(&self.root)),
            Kind::Files => match FilesView::new(self.root.clone()) {
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

    /// Show `path` in an editor pane, reusing one that already has it open.
    fn open_file(&mut self, path: PathBuf, edit: bool) {
        let existing = self
            .panes
            .iter()
            .find_map(|(id, p)| matches!(p, Pane::Editor(v) if v.path == path).then_some(*id));
        match existing {
            Some(id) => self.focus = id,
            None => self.open(Kind::Editor(path), Dir::Horizontal),
        }
        if let Some(Pane::Editor(v)) = self.panes.get_mut(&self.focus) {
            if edit {
                v.mode = Mode::Edit;
            }
        }
    }

    /// Close a pane, but make the user confirm once if an editor has unsaved changes.
    fn request_close(&mut self, id: PaneId) {
        let unsaved = matches!(self.panes.get(&id), Some(Pane::Editor(v)) if v.dirty);
        if unsaved && self.pending_close != Some(id) {
            self.pending_close = Some(id);
            self.notice = Some("unsaved changes: close again to discard them".to_string());
            return;
        }
        self.pending_close = None;
        self.close(id);
    }

    /// Pane ids in layout order.
    pub fn pane_order(&self) -> Vec<PaneId> {
        let mut ids = Vec::new();
        self.tree.ids(&mut ids);
        ids
    }

    fn close(&mut self, id: PaneId) {
        let mut ids = Vec::new();
        self.tree.ids(&mut ids);
        if ids.len() <= 1 {
            self.quit = true;
            return;
        }
        self.tree.remove(id);
        self.panes.remove(&id);
        if self.zoomed == Some(id) {
            self.zoomed = None;
        }
        if self.sel.as_ref().is_some_and(|s| s.pane == id) {
            self.sel = None;
        }
        if self.focus == id {
            let mut ids = Vec::new();
            self.tree.ids(&mut ids);
            self.focus = ids.first().copied().unwrap_or(0);
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

    /// Jump to the n-th pane (1-based, in dock order).
    fn goto_pane(&mut self, n: u8) {
        match self.pane_order().get(n as usize - 1) {
            Some(&id) => {
                self.focus = id;
                if self.zoomed.is_some() {
                    self.zoomed = Some(id);
                }
                self.sel = None;
            }
            None => self.notice = Some(format!("there is no pane {n}")),
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
        if !self.prefix && !is_prefix_key {
            self.pending_close = None;
        }
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
            Some(Pane::Graph(v)) => v.key(key.code),
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

    fn run_menu(&mut self, target: PaneId, action: MenuAction) {
        self.focus = target;
        match action {
            MenuAction::SplitRight => self.open(Kind::Terminal, Dir::Horizontal),
            MenuAction::SplitDown => self.open(Kind::Terminal, Dir::Vertical),
            MenuAction::NewGraph => self.open(Kind::Graph, Dir::Horizontal),
            MenuAction::NewFiles => self.open(Kind::Files, Dir::Horizontal),
            MenuAction::Zoom => self.toggle_zoom(),
            MenuAction::Close => self.request_close(target),
            MenuAction::Keybindings => self.run_action(Action::OpenKeybinds),
            MenuAction::Settings => self.run_action(Action::OpenSettings),
            MenuAction::Themes => self.run_action(Action::OpenThemes),
            MenuAction::Detach => self.detach = true,
            MenuAction::QuitSession => self.quit = true,
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

        // A dialog owns the screen: the wheel scrolls it, everything else is ignored.
        if let Some(overlay) = &mut self.overlay {
            match m.kind {
                MouseEventKind::ScrollUp => overlay.scroll(-3),
                MouseEventKind::ScrollDown => overlay.scroll(3),
                _ => {}
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
                if let Some(DockHit::Pane(id)) = self.dock_at(x, y) {
                    self.focus = id;
                    self.menu = Some(Menu::new(x, y, id));
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
                        DockHit::Pane(id) => {
                            self.focus = id;
                            if self.zoomed.is_some() {
                                self.zoomed = Some(id);
                            }
                        }
                        DockHit::New => self.open(Kind::Terminal, Dir::Horizontal),
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
            Some(Pane::Graph(v)) => v.scroll(-dir),
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
                if let Some(row) = y.checked_sub(area.y) {
                    let idx = v.state.offset() + row as usize;
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
