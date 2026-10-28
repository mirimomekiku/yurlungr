use std::path::{Path, PathBuf};

use anyhow::Result;
use crossterm::event::KeyCode;
use ratatui::widgets::ListState;

use crate::{
    file_browser::FileBrowser,
    git_graph::{self, GraphRow},
    terminal::{Status, Terminal},
};

pub enum Pane {
    Terminal(Terminal),
    Graph(GraphView),
    Files(FilesView),
}

impl Pane {
    pub fn status(&self) -> Option<Status> {
        match self {
            Pane::Terminal(t) => Some(t.status()),
            _ => None,
        }
    }

    pub fn agent(&self) -> Option<&str> {
        match self {
            Pane::Terminal(t) => t.agent(),
            _ => None,
        }
    }

    pub fn title(&self) -> String {
        match self {
            Pane::Terminal(_) => " terminal ".to_string(),
            Pane::Graph(_) => " git graph ".to_string(),
            Pane::Files(v) => format!(" {} ", v.fb.dir.display()),
        }
    }
}

fn step(state: &mut ListState, len: usize, delta: isize) {
    if len == 0 {
        return;
    }
    let cur = state.selected().unwrap_or(0) as isize;
    state.select(Some((cur + delta).clamp(0, len as isize - 1) as usize));
}

pub struct GraphView {
    root: PathBuf,
    pub rows: Result<Vec<GraphRow>, String>,
    pub state: ListState,
}

impl GraphView {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            rows: git_graph::load(root, 500).map_err(|e| e.to_string()),
            state: ListState::default().with_selected(Some(0)),
        }
    }

    fn len(&self) -> usize {
        self.rows.as_ref().map_or(0, Vec::len)
    }

    pub fn scroll(&mut self, delta: isize) {
        step(&mut self.state, self.len(), delta);
    }

    pub fn select(&mut self, idx: usize) {
        if idx < self.len() {
            self.state.select(Some(idx));
        }
    }

    pub fn key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Down | KeyCode::Char('j') => self.scroll(1),
            KeyCode::Up | KeyCode::Char('k') => self.scroll(-1),
            KeyCode::PageDown => self.scroll(10),
            KeyCode::PageUp => self.scroll(-10),
            KeyCode::Home | KeyCode::Char('g') => self.state.select(Some(0)),
            KeyCode::End | KeyCode::Char('G') => self.scroll(isize::MAX / 2),
            KeyCode::Char('r') => {
                self.rows = git_graph::load(&self.root, 500).map_err(|e| e.to_string());
                self.state.select(Some(0));
            }
            _ => {}
        }
    }
}

pub struct FilesView {
    pub fb: FileBrowser,
    pub state: ListState,
}

impl FilesView {
    pub fn new(root: PathBuf) -> Result<Self> {
        Ok(Self {
            fb: FileBrowser::new(root)?,
            state: ListState::default().with_selected(Some(0)),
        })
    }

    pub fn scroll(&mut self, delta: isize) {
        step(&mut self.state, self.fb.entries.len(), delta);
    }

    fn enter(&mut self) -> Result<()> {
        let idx = self.state.selected().unwrap_or(0);
        if self.fb.enter(idx)? {
            self.state.select(Some(0));
        }
        Ok(())
    }

    fn up(&mut self) -> Result<()> {
        if self.fb.parent()? {
            self.state.select(Some(0));
        }
        Ok(())
    }

    /// Click selects an entry; clicking the selected entry opens it.
    pub fn click(&mut self, idx: usize) -> Result<()> {
        if idx >= self.fb.entries.len() {
            return Ok(());
        }
        if self.state.selected() == Some(idx) {
            self.enter()
        } else {
            self.state.select(Some(idx));
            Ok(())
        }
    }

    pub fn key(&mut self, code: KeyCode) -> Result<()> {
        match code {
            KeyCode::Down | KeyCode::Char('j') => self.scroll(1),
            KeyCode::Up | KeyCode::Char('k') => self.scroll(-1),
            KeyCode::PageDown => self.scroll(10),
            KeyCode::PageUp => self.scroll(-10),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => self.enter()?,
            KeyCode::Left | KeyCode::Backspace | KeyCode::Char('h') => self.up()?,
            _ => {}
        }
        Ok(())
    }
}
