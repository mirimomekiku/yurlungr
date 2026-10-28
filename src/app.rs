use std::{collections::HashMap, fs, path::PathBuf};

use anyhow::Result;
use base64::{engine::general_purpose::STANDARD, Engine};
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::{layout::Rect, widgets::Block};
use serde::{Deserialize, Serialize};

use crate::{
    keys::key_bytes,
    layout::{Dir, Divider, Node, PaneId},
    menu::{Menu, MenuAction},
    mouse,
    proto::ReportState,
    terminal::{Status, Terminal},
    views::{FilesView, GraphView, Pane},
};

#[derive(Clone, Copy, Serialize, Deserialize)]
pub enum Kind {
    Terminal,
    Graph,
    Files,
}

fn kind_of(p: &Pane) -> Kind {
    match p {
        Pane::Terminal(_) => Kind::Terminal,
        Pane::Graph(_) => Kind::Graph,
        Pane::Files(_) => Kind::Files,
    }
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
    /// True after the prefix key (Ctrl+B) until the next key press.
    pub prefix: bool,
    pub menu: Option<Menu>,
    pub notice: Option<String>,
    pub rects: Vec<(PaneId, Rect)>,
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
        Self {
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
                Kind::Terminal => {
                    Terminal::spawn(&saved.root, 24, 80, &pane_env(session, id)).ok().map(Pane::Terminal)
                }
                Kind::Graph => Some(Pane::Graph(GraphView::new(&saved.root))),
                Kind::Files => FilesView::new(saved.root.clone()).ok().map(Pane::Files),
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
        let id = self.next_id;
        let pane = match kind {
            Kind::Terminal => match Terminal::spawn(&self.root, 24, 80, &pane_env(&self.session, id)) {
                Ok(t) => Pane::Terminal(t),
                Err(e) => {
                    self.notice = Some(format!("failed to start shell: {e}"));
                    return;
                }
            },
            Kind::Graph => Pane::Graph(GraphView::new(&self.root)),
            Kind::Files => match FilesView::new(self.root.clone()) {
                Ok(v) => Pane::Files(v),
                Err(e) => {
                    self.notice = Some(format!("failed to open files: {e}"));
                    return;
                }
            },
        };
        self.next_id += 1;
        if self.tree.split(self.focus, id, dir) {
            self.panes.insert(id, pane);
            self.focus = id;
            self.zoomed = None;
            self.sel = None;
        }
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

    fn cycle_focus(&mut self) {
        let mut ids = Vec::new();
        self.tree.ids(&mut ids);
        if let Some(i) = ids.iter().position(|&id| id == self.focus) {
            self.focus = ids[(i + 1) % ids.len()];
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
            Event::Paste(s) => {
                if let Some(Pane::Terminal(t)) = self.panes.get(&self.focus) {
                    let bracketed = t.with_screen(|s| s.bracketed_paste());
                    if bracketed {
                        t.write(b"\x1b[200~");
                    }
                    t.write(s.as_bytes());
                    if bracketed {
                        t.write(b"\x1b[201~");
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn on_key(&mut self, key: KeyEvent) -> Result<()> {
        self.notice = None;
        self.sel = None;

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

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if self.prefix {
            self.prefix = false;
            if ctrl && key.code == KeyCode::Char('b') {
                self.send_to_focus(&[0x02]);
            } else {
                self.on_prefix(key.code);
            }
            return Ok(());
        }
        if ctrl && key.code == KeyCode::Char('b') {
            self.prefix = true;
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
                if let Err(e) = v.key(key.code) {
                    self.notice = Some(e.to_string());
                }
            }
            None => {}
        }
        Ok(())
    }

    fn send_to_focus(&mut self, bytes: &[u8]) {
        if let Some(Pane::Terminal(t)) = self.panes.get(&self.focus) {
            t.write(bytes);
        }
    }

    fn on_prefix(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('%' | '|' | 'v') => self.open(Kind::Terminal, Dir::Horizontal),
            KeyCode::Char('"' | '-' | 's') => self.open(Kind::Terminal, Dir::Vertical),
            KeyCode::Char('c') => self.open(Kind::Terminal, Dir::Horizontal),
            KeyCode::Char('g') => self.open(Kind::Graph, Dir::Horizontal),
            KeyCode::Char('f') => self.open(Kind::Files, Dir::Horizontal),
            KeyCode::Char('x') => self.close(self.focus),
            KeyCode::Char('z') => self.toggle_zoom(),
            KeyCode::Char('o') | KeyCode::Tab => self.cycle_focus(),
            KeyCode::Left | KeyCode::Char('h') => self.focus_dir(-1, 0),
            KeyCode::Right | KeyCode::Char('l') => self.focus_dir(1, 0),
            KeyCode::Up | KeyCode::Char('k') => self.focus_dir(0, -1),
            KeyCode::Down | KeyCode::Char('j') => self.focus_dir(0, 1),
            KeyCode::Char('H') => self.tree.nudge(self.focus, Dir::Horizontal, -0.05),
            KeyCode::Char('L') => self.tree.nudge(self.focus, Dir::Horizontal, 0.05),
            KeyCode::Char('K') => self.tree.nudge(self.focus, Dir::Vertical, -0.05),
            KeyCode::Char('J') => self.tree.nudge(self.focus, Dir::Vertical, 0.05),
            KeyCode::Char('d') => self.detach = true,
            KeyCode::Char('q') => self.quit = true,
            _ => {}
        }
    }

    fn run_menu(&mut self, target: PaneId, action: MenuAction) {
        self.focus = target;
        match action {
            MenuAction::SplitRight => self.open(Kind::Terminal, Dir::Horizontal),
            MenuAction::SplitDown => self.open(Kind::Terminal, Dir::Vertical),
            MenuAction::NewGraph => self.open(Kind::Graph, Dir::Horizontal),
            MenuAction::NewFiles => self.open(Kind::Files, Dir::Horizontal),
            MenuAction::Zoom => self.toggle_zoom(),
            MenuAction::Close => self.close(target),
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
        if let Some(bytes) = mouse::encode(
            m.kind,
            m.modifiers,
            m.column - area.x,
            m.row - area.y,
            mode,
            enc,
        ) {
            t.write(&bytes);
        }
        if matches!(m.kind, MouseEventKind::Down(_)) {
            self.focus = id;
        }
        true
    }

    fn on_mouse(&mut self, m: MouseEvent) -> Result<()> {
        let (x, y) = (m.column, m.row);

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
                if let Some(id) = self.pane_at(x, y) {
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
            None => {}
        }
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
                    v.click(idx)?;
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
