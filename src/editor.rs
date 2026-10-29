//! A small modal text viewer/editor pane. Files open read-only (view mode);
//! `e` switches to edit mode, `Esc` goes back, `Ctrl+S` saves.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthChar;

use crate::keymap::Action;

const MAX_BYTES: u64 = 2 * 1024 * 1024;
const TAB: usize = 4;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    View,
    Edit,
}

pub struct EditorView {
    pub path: PathBuf,
    pub lines: Vec<String>,
    /// Cursor line and character index within it.
    pub row: usize,
    pub col: usize,
    /// First visible line and first visible display column.
    pub top: usize,
    pub left: usize,
    pub mode: Mode,
    pub dirty: bool,
    crlf: bool,
    final_newline: bool,
    /// The file had no content; the first edit gives it a trailing newline.
    fresh_empty: bool,
    /// Size of the text area, set by the renderer each frame.
    pub height: usize,
    pub width: usize,
}

/// Display width of `ch` when it starts at display column `x`. Control
/// characters are drawn as a one-cell placeholder so a file can never inject
/// terminal escape sequences through the renderer.
pub fn char_width(ch: char, x: usize) -> usize {
    match ch {
        '\t' => TAB - x % TAB,
        c if c.is_control() => 1,
        c => c.width().unwrap_or(0),
    }
}

pub fn display_col(line: &str, col: usize) -> usize {
    line.chars().take(col).fold(0, |x, c| x + char_width(c, x))
}

/// The part of `line` that falls in display columns `left..left + width`.
pub fn visible_slice(line: &str, left: usize, width: usize) -> String {
    let mut out = String::new();
    let mut x = 0;
    for ch in line.chars() {
        let w = char_width(ch, x);
        if x >= left + width {
            break;
        }
        if x + w > left {
            if x < left {
                out.push_str(&" ".repeat(x + w - left));
            } else if ch == '\t' {
                out.push_str(&" ".repeat(w));
            } else if ch.is_control() {
                out.push('·');
            } else {
                out.push(ch);
            }
        }
        x += w;
    }
    out
}

fn byte_idx(line: &str, col: usize) -> usize {
    line.char_indices().nth(col).map_or(line.len(), |(i, _)| i)
}

impl EditorView {
    pub fn open(path: &Path) -> Result<Self> {
        let meta = fs::metadata(path).with_context(|| format!("cannot open {}", path.display()))?;
        if !meta.is_file() {
            bail!("not a regular file");
        }
        if meta.len() > MAX_BYTES {
            bail!(
                "file is too large to edit ({} KB, limit {} KB)",
                meta.len() / 1024,
                MAX_BYTES / 1024
            );
        }
        let bytes = fs::read(path)?;
        if bytes.contains(&0) {
            bail!("binary file");
        }
        let text = String::from_utf8(bytes).map_err(|_| anyhow::anyhow!("not valid UTF-8"))?;

        let crlf = text.contains("\r\n");
        let final_newline = text.ends_with('\n');
        let mut lines: Vec<String> =
            text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l).to_string()).collect();
        if text.ends_with('\n') {
            lines.pop();
        }
        if lines.is_empty() {
            lines.push(String::new());
        }
        Ok(Self {
            path: path.to_path_buf(),
            lines,
            row: 0,
            col: 0,
            top: 0,
            left: 0,
            mode: Mode::View,
            dirty: false,
            crlf,
            final_newline,
            fresh_empty: text.is_empty(),
            height: 20,
            width: 80,
        })
    }

    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map_or_else(|| self.path.display().to_string(), |n| n.to_string_lossy().into_owned())
    }

    pub fn contents(&self) -> String {
        let nl = if self.crlf { "\r\n" } else { "\n" };
        let mut text = self.lines.join(nl);
        if self.final_newline {
            text.push_str(nl);
        }
        text
    }

    pub fn save(&mut self) -> Result<()> {
        crate::file_browser::write_atomic(&self.path, self.contents().as_bytes())?;
        self.dirty = false;
        Ok(())
    }

    fn reload(&mut self) -> Result<()> {
        let fresh = Self::open(&self.path)?;
        let (row, col, mode) = (self.row, self.col, self.mode);
        *self = Self { height: self.height, width: self.width, ..fresh };
        self.mode = mode;
        self.row = row.min(self.lines.len() - 1);
        self.col = col.min(self.line_len());
        Ok(())
    }

    fn line_len(&self) -> usize {
        self.lines[self.row].chars().count()
    }

    /// Width of the line-number column, including its trailing space.
    pub fn gutter(&self) -> usize {
        self.lines.len().to_string().len() + 1
    }

    pub fn set_viewport(&mut self, height: usize, width: usize) {
        self.height = height.max(1);
        self.width = width.max(1);
    }

    /// Scroll the cursor into view.
    pub fn ensure_visible(&mut self) {
        if self.row < self.top {
            self.top = self.row;
        } else if self.row >= self.top + self.height {
            self.top = self.row + 1 - self.height;
        }
        let x = display_col(&self.lines[self.row], self.col);
        if x < self.left {
            self.left = x;
        } else if x >= self.left + self.width {
            self.left = x + 1 - self.width;
        }
    }

    /// Scroll the view (not the cursor); positive moves towards the end.
    pub fn scroll_view(&mut self, delta: isize) {
        let max = self.lines.len().saturating_sub(1) as isize;
        self.top = (self.top as isize + delta).clamp(0, max) as usize;
        self.row = self.row.clamp(self.top, (self.top + self.height - 1).min(self.lines.len() - 1));
        self.col = self.col.min(self.line_len());
    }

    /// Place the cursor at a text-area position (relative to the first visible cell).
    pub fn click(&mut self, y: usize, x: usize) {
        self.row = (self.top + y).min(self.lines.len() - 1);
        let target = self.left + x;
        let mut width = 0;
        let mut col = 0;
        for ch in self.lines[self.row].chars() {
            let w = char_width(ch, width);
            if width + w > target {
                break;
            }
            width += w;
            col += 1;
        }
        self.col = col;
    }

    fn move_row(&mut self, delta: isize) {
        self.row = (self.row as isize + delta).clamp(0, self.lines.len() as isize - 1) as usize;
        self.col = self.col.min(self.line_len());
    }

    fn first_edit(&mut self) {
        if self.fresh_empty {
            self.fresh_empty = false;
            self.final_newline = true;
        }
    }

    fn insert_str(&mut self, text: &str) {
        self.first_edit();
        for ch in text.chars() {
            match ch {
                '\n' => self.split_line(),
                '\r' => {}
                c => {
                    let at = byte_idx(&self.lines[self.row], self.col);
                    self.lines[self.row].insert(at, c);
                    self.col += 1;
                }
            }
        }
        self.dirty = true;
    }

    pub fn paste(&mut self, text: &str) {
        if self.mode == Mode::Edit {
            self.insert_str(text);
        }
    }

    fn split_line(&mut self) {
        self.first_edit();
        let at = byte_idx(&self.lines[self.row], self.col);
        let rest = self.lines[self.row].split_off(at);
        self.row += 1;
        self.lines.insert(self.row, rest);
        self.col = 0;
        self.dirty = true;
    }

    fn backspace(&mut self) {
        if self.col > 0 {
            let at = byte_idx(&self.lines[self.row], self.col - 1);
            self.lines[self.row].remove(at);
            self.col -= 1;
        } else if self.row > 0 {
            let line = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.line_len();
            self.lines[self.row].push_str(&line);
        } else {
            return;
        }
        self.dirty = true;
    }

    fn delete(&mut self) {
        if self.col < self.line_len() {
            let at = byte_idx(&self.lines[self.row], self.col);
            self.lines[self.row].remove(at);
        } else if self.row + 1 < self.lines.len() {
            let next = self.lines.remove(self.row + 1);
            self.lines[self.row].push_str(&next);
        } else {
            return;
        }
        self.dirty = true;
    }

    /// Whether `action` means something in the current mode. Anything else is
    /// treated as ordinary input, so bound keys still type in edit mode.
    pub fn accepts(&self, action: Action) -> bool {
        match action {
            Action::EditorSave => true,
            Action::EditorStartEdit | Action::EditorReload => self.mode == Mode::View,
            Action::EditorStopEdit => self.mode == Mode::Edit,
            _ => false,
        }
    }

    /// Returns a message to show in the status line, if any. `cmd` is the
    /// configured action the key resolved to, already filtered by `accepts`.
    pub fn key(&mut self, key: KeyEvent, cmd: Option<Action>) -> Result<Option<String>> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let page = self.height.saturating_sub(1).max(1) as isize;

        match cmd {
            Some(Action::EditorSave) => {
                self.save()?;
                return Ok(Some(format!("saved {}", self.name())));
            }
            Some(Action::EditorStartEdit) => {
                self.mode = Mode::Edit;
                return Ok(Some(
                    "editing: stop with the stop-editing key, save with the save key".to_string(),
                ));
            }
            Some(Action::EditorStopEdit) => {
                self.mode = Mode::View;
                return Ok(None);
            }
            Some(Action::EditorReload) => {
                if self.dirty {
                    return Ok(Some(
                        "unsaved changes: save first, or close and reopen".to_string(),
                    ));
                }
                self.reload()?;
                return Ok(Some("reloaded".to_string()));
            }
            _ => {}
        }

        match self.mode {
            Mode::View => match key.code {
                KeyCode::Down | KeyCode::Char('j') => self.move_row(1),
                KeyCode::Up | KeyCode::Char('k') => self.move_row(-1),
                KeyCode::PageDown => self.move_row(page),
                KeyCode::PageUp => self.move_row(-page),
                KeyCode::Home | KeyCode::Char('g') => {
                    self.row = 0;
                    self.col = 0;
                }
                KeyCode::End | KeyCode::Char('G') => self.move_row(isize::MAX / 2),
                KeyCode::Left | KeyCode::Char('h') => self.col = self.col.saturating_sub(1),
                KeyCode::Right | KeyCode::Char('l') => {
                    self.col = (self.col + 1).min(self.line_len())
                }
                _ => {}
            },
            Mode::Edit => match key.code {
                KeyCode::Char(c) if !ctrl && !alt => self.insert_str(c.encode_utf8(&mut [0; 4])),
                KeyCode::Enter => self.split_line(),
                KeyCode::Tab => self.insert_str(&" ".repeat(TAB)),
                KeyCode::Backspace => self.backspace(),
                KeyCode::Delete => self.delete(),
                KeyCode::Left => {
                    if self.col > 0 {
                        self.col -= 1;
                    } else if self.row > 0 {
                        self.row -= 1;
                        self.col = self.line_len();
                    }
                }
                KeyCode::Right => {
                    if self.col < self.line_len() {
                        self.col += 1;
                    } else if self.row + 1 < self.lines.len() {
                        self.row += 1;
                        self.col = 0;
                    }
                }
                KeyCode::Up => self.move_row(-1),
                KeyCode::Down => self.move_row(1),
                KeyCode::PageUp => self.move_row(-page),
                KeyCode::PageDown => self.move_row(page),
                KeyCode::Home => self.col = 0,
                KeyCode::End => self.col = self.line_len(),
                _ => {}
            },
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor(text: &str) -> (tempfile::TempDir, EditorView) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        fs::write(&path, text).unwrap();
        (dir, EditorView::open(&path).unwrap())
    }

    /// Press a key the way the app does: resolve it through the default keymap first.
    fn press_with(v: &mut EditorView, code: KeyCode, mods: KeyModifiers) {
        use crate::keymap::{KeyCombo, Keymap, Scope};
        let ev = KeyEvent::new(code, mods);
        let cmd = Keymap::default()
            .lookup(false, KeyCombo::from_event(ev), Scope::Editor)
            .filter(|a| v.accepts(*a));
        v.key(ev, cmd).unwrap();
    }

    fn press(v: &mut EditorView, code: KeyCode) {
        press_with(v, code, KeyModifiers::NONE);
    }

    #[test]
    fn round_trips_line_endings_and_final_newline() {
        for text in ["a\nb\n", "a\nb", "a\r\nb\r\n", "", "\n"] {
            let (_d, v) = editor(text);
            assert_eq!(v.contents(), text, "{text:?}");
        }
    }

    #[test]
    fn rejects_binary_and_non_utf8() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("b");
        fs::write(&p, b"ab\0cd").unwrap();
        assert!(EditorView::open(&p).is_err());
        fs::write(&p, [0xff, 0xfe, 0x41]).unwrap();
        assert!(EditorView::open(&p).is_err());
    }

    #[test]
    fn view_mode_does_not_modify() {
        let (_d, mut v) = editor("abc\n");
        press(&mut v, KeyCode::Char('x'));
        assert!(!v.dirty);
        assert_eq!(v.contents(), "abc\n");
    }

    #[test]
    fn edits_split_join_and_save() {
        let (_d, mut v) = editor("hello\nworld\n");
        press(&mut v, KeyCode::Char('e'));
        press(&mut v, KeyCode::End);
        press(&mut v, KeyCode::Char('!'));
        press(&mut v, KeyCode::Enter);
        press(&mut v, KeyCode::Char('é'));
        assert_eq!(v.contents(), "hello!\né\nworld\n");
        press(&mut v, KeyCode::Backspace);
        press(&mut v, KeyCode::Backspace);
        assert_eq!(v.contents(), "hello!\nworld\n");
        press_with(&mut v, KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert!(!v.dirty);
        assert_eq!(fs::read_to_string(&v.path).unwrap(), v.contents());
    }

    #[test]
    fn control_characters_never_reach_the_screen() {
        assert_eq!(visible_slice("a\x1b[31mb", 0, 20), "a·[31mb");
        assert_eq!(visible_slice("\tx", 0, 20), "    x");
        assert_eq!(display_col("\tx", 1), 4);
        assert_eq!(visible_slice("abcdef", 2, 3), "cde");
    }

    #[test]
    fn click_maps_display_columns_to_characters() {
        let (_d, mut v) = editor("\tab\n");
        v.click(0, 5);
        assert_eq!(v.col, 2);
    }

    #[test]
    fn bound_keys_still_type_in_edit_mode() {
        let (_d, mut v) = editor("");
        press(&mut v, KeyCode::Char('e')); // bound to "start editing" in view mode
        assert_eq!(v.mode, Mode::Edit);
        press(&mut v, KeyCode::Char('e')); // now just a letter
        press(&mut v, KeyCode::Char('i'));
        press(&mut v, KeyCode::Enter); // a newline, not "start editing"
        assert_eq!(v.contents(), "ei\n\n");
        press(&mut v, KeyCode::Esc);
        assert_eq!(v.mode, Mode::View);
        press(&mut v, KeyCode::Char('x'));
        assert_eq!(v.contents(), "ei\n\n", "view mode never types");
    }
}
