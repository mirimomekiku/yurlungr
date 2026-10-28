use std::path::{Path, PathBuf};

use anyhow::Result;
use crossterm::event::KeyCode;
use ratatui::widgets::ListState;

use crate::{
    editor::{EditorView, Mode},
    file_browser::{self, FileBrowser},
    git_graph::{self, GraphRow},
    keymap::Action,
    terminal::{Status, Terminal},
};

pub enum Pane {
    Terminal(Terminal),
    Graph(GraphView),
    Files(FilesView),
    Editor(EditorView),
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
            Pane::Editor(v) => format!(
                " {}{} · {} ",
                v.path.display(),
                if v.dirty { " [+]" } else { "" },
                if v.mode == Mode::Edit { "EDIT" } else { "view" }
            ),
        }
    }

    /// Short name for the dock.
    pub fn label(&self) -> String {
        match self {
            Pane::Terminal(t) => t.agent().unwrap_or("shell").to_string(),
            Pane::Graph(_) => "graph".to_string(),
            Pane::Files(v) => {
                v.fb.dir
                    .file_name()
                    .map_or_else(|| "files".to_string(), |n| n.to_string_lossy().into_owned())
            }
            Pane::Editor(v) => format!("{}{}", v.name(), if v.dirty { "*" } else { "" }),
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
        let len = self.len();
        step(&mut self.state, len, delta);
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

/// A file marked for copy or cut; shared by every file browser in the session.
pub struct Clip {
    pub path: PathBuf,
    pub cut: bool,
}

pub enum PromptKind {
    NewFile,
    NewDir,
    Rename(PathBuf),
    Move(PathBuf),
    Delete(PathBuf),
}

/// A one-line input (or yes/no question) shown at the bottom of a file browser.
pub struct Prompt {
    pub kind: PromptKind,
    pub text: String,
}

impl Prompt {
    pub fn label(&self) -> String {
        match &self.kind {
            PromptKind::NewFile => "new file".to_string(),
            PromptKind::NewDir => "new folder".to_string(),
            PromptKind::Rename(_) => "rename to".to_string(),
            PromptKind::Move(_) => "move to".to_string(),
            PromptKind::Delete(p) => format!(
                "delete '{}'? (y/n)",
                p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
            ),
        }
    }

    pub fn is_confirm(&self) -> bool {
        matches!(self.kind, PromptKind::Delete(_))
    }
}

/// What the app should do after a file browser handled a key.
pub enum FilesAct {
    None,
    Open { path: PathBuf, edit: bool },
    Msg(String),
}

pub struct FilesView {
    pub fb: FileBrowser,
    pub state: ListState,
    pub prompt: Option<Prompt>,
}

impl FilesView {
    pub fn new(root: PathBuf) -> Result<Self> {
        Ok(Self {
            fb: FileBrowser::new(root)?,
            state: ListState::default().with_selected(Some(0)),
            prompt: None,
        })
    }

    pub fn scroll(&mut self, delta: isize) {
        step(&mut self.state, self.fb.entries.len(), delta);
    }

    fn selected_path(&self) -> Option<PathBuf> {
        self.fb.path_at(self.state.selected()?)
    }

    /// Reload the listing and put the selection on `name` if it is there.
    fn refresh(&mut self, name: Option<&str>) -> Result<()> {
        self.fb.reload()?;
        let keep = self.state.selected().unwrap_or(0);
        let idx = name.and_then(|n| self.fb.index_of(n)).unwrap_or(keep);
        self.state.select(Some(idx.min(self.fb.entries.len().saturating_sub(1))));
        Ok(())
    }

    fn enter(&mut self) -> Result<FilesAct> {
        let idx = self.state.selected().unwrap_or(0);
        if self.fb.enter(idx)? {
            self.state.select(Some(0));
            return Ok(FilesAct::None);
        }
        Ok(match self.fb.path_at(idx) {
            Some(path) => FilesAct::Open { path, edit: false },
            None => FilesAct::None,
        })
    }

    fn up(&mut self) -> Result<()> {
        let came_from = self.fb.dir.file_name().map(|n| n.to_string_lossy().into_owned());
        if self.fb.parent()? {
            self.state.select(Some(0));
            if let Some(name) = came_from {
                self.refresh(Some(&name))?;
            }
        }
        Ok(())
    }

    /// Click selects an entry; clicking the selected entry opens it.
    pub fn click(&mut self, idx: usize) -> Result<FilesAct> {
        if idx >= self.fb.entries.len() {
            return Ok(FilesAct::None);
        }
        if self.state.selected() == Some(idx) {
            self.enter()
        } else {
            self.state.select(Some(idx));
            Ok(FilesAct::None)
        }
    }

    pub fn paste_text(&mut self, text: &str) {
        if let Some(p) = &mut self.prompt {
            if !p.is_confirm() {
                p.text.extend(text.chars().filter(|c| !c.is_control()));
            }
        }
    }

    /// Handle a key. `cmd` is the configured action the key resolved to (never
    /// set while a prompt is open, so typing is never mistaken for a command);
    /// `code` drives navigation and the prompt.
    pub fn key(
        &mut self,
        code: KeyCode,
        cmd: Option<Action>,
        clip: &mut Option<Clip>,
    ) -> Result<FilesAct> {
        if self.prompt.is_some() {
            return self.prompt_key(code, clip);
        }
        let name_of = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned());
        match cmd {
            Some(Action::FileEdit) => {
                return Ok(match self.selected_path() {
                    Some(path) if path.is_dir() => FilesAct::Msg("that is a folder".to_string()),
                    Some(path) => FilesAct::Open { path, edit: true },
                    None => FilesAct::None,
                });
            }
            Some(Action::FileNewFile) => {
                self.prompt = Some(Prompt { kind: PromptKind::NewFile, text: String::new() });
            }
            Some(Action::FileNewDir) => {
                self.prompt = Some(Prompt { kind: PromptKind::NewDir, text: String::new() });
            }
            Some(Action::FileRename) => {
                if let Some(path) = self.selected_path() {
                    let text = name_of(&path).unwrap_or_default();
                    self.prompt = Some(Prompt { kind: PromptKind::Rename(path), text });
                }
            }
            Some(Action::FileMove) => {
                if let Some(path) = self.selected_path() {
                    let text = format!("{}/", self.fb.dir.display());
                    self.prompt = Some(Prompt { kind: PromptKind::Move(path), text });
                }
            }
            Some(Action::FileDelete) => {
                if let Some(path) = self.selected_path() {
                    self.prompt =
                        Some(Prompt { kind: PromptKind::Delete(path), text: String::new() });
                }
            }
            Some(a @ (Action::FileCopy | Action::FileCut)) => {
                if let Some(path) = self.selected_path() {
                    let cut = a == Action::FileCut;
                    let name = name_of(&path).unwrap_or_default();
                    *clip = Some(Clip { path, cut });
                    return Ok(FilesAct::Msg(format!(
                        "{} '{name}'; paste to finish",
                        if cut { "cut" } else { "copied" }
                    )));
                }
            }
            Some(Action::FilePaste) => {
                let Some(c) = clip.as_ref() else {
                    return Ok(FilesAct::Msg("nothing copied or cut".to_string()));
                };
                let dir = self.fb.dir.clone();
                let dst = if c.cut {
                    file_browser::move_to(&c.path, &dir.to_string_lossy(), &dir)?
                } else {
                    file_browser::copy_into(&c.path, &dir)?
                };
                if c.cut {
                    *clip = None;
                }
                let name = name_of(&dst).unwrap_or_default();
                self.refresh(Some(&name))?;
                return Ok(FilesAct::Msg(format!("pasted '{name}'")));
            }
            Some(Action::FileToggleHidden) => {
                self.fb.show_hidden = !self.fb.show_hidden;
                self.refresh(None)?;
                self.state.select(Some(0));
                return Ok(FilesAct::Msg(
                    if self.fb.show_hidden {
                        "showing hidden files"
                    } else {
                        "hiding hidden files"
                    }
                    .to_string(),
                ));
            }
            Some(Action::FileRefresh) => self.refresh(None)?,
            Some(_) => {}
            None => match code {
                KeyCode::Down | KeyCode::Char('j') => self.scroll(1),
                KeyCode::Up | KeyCode::Char('k') => self.scroll(-1),
                KeyCode::PageDown => self.scroll(10),
                KeyCode::PageUp => self.scroll(-10),
                KeyCode::Home => self.state.select(Some(0)),
                KeyCode::End | KeyCode::Char('G') => self.scroll(isize::MAX / 2),
                KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => return self.enter(),
                KeyCode::Left | KeyCode::Backspace | KeyCode::Char('h') => self.up()?,
                _ => {}
            },
        }
        Ok(FilesAct::None)
    }

    fn prompt_key(&mut self, code: KeyCode, clip: &mut Option<Clip>) -> Result<FilesAct> {
        let Some(prompt) = self.prompt.as_mut() else { return Ok(FilesAct::None) };
        if prompt.is_confirm() {
            let yes = matches!(code, KeyCode::Char('y' | 'Y') | KeyCode::Enter);
            let Some(Prompt { kind: PromptKind::Delete(path), .. }) = self.prompt.take() else {
                return Ok(FilesAct::None);
            };
            if !yes {
                return Ok(FilesAct::None);
            }
            file_browser::delete(&path)?;
            if clip.as_ref().is_some_and(|c| c.path == path) {
                *clip = None;
            }
            self.refresh(None)?;
            return Ok(FilesAct::Msg("deleted".to_string()));
        }

        match code {
            KeyCode::Esc => self.prompt = None,
            KeyCode::Backspace => {
                prompt.text.pop();
            }
            KeyCode::Char(c) => prompt.text.push(c),
            KeyCode::Enter => {
                let Some(Prompt { kind, text }) = self.prompt.take() else {
                    return Ok(FilesAct::None);
                };
                let dir = self.fb.dir.clone();
                let created = match kind {
                    PromptKind::NewFile => file_browser::create_file(&dir, text.trim())?,
                    PromptKind::NewDir => file_browser::create_dir(&dir, text.trim())?,
                    PromptKind::Rename(path) => {
                        let dst = file_browser::rename(&path, text.trim())?;
                        if clip.as_ref().is_some_and(|c| c.path == path) {
                            *clip = None;
                        }
                        dst
                    }
                    PromptKind::Move(path) => {
                        let dst = file_browser::move_to(&path, &text, &dir)?;
                        if clip.as_ref().is_some_and(|c| c.path == path) {
                            *clip = None;
                        }
                        dst
                    }
                    PromptKind::Delete(_) => return Ok(FilesAct::None),
                };
                let name = created.file_name().map(|n| n.to_string_lossy().into_owned());
                self.refresh(name.as_deref())?;
                return Ok(FilesAct::Msg(format!("done: {}", created.display())));
            }
            _ => {}
        }
        Ok(FilesAct::None)
    }
}
