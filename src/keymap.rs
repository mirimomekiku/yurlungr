//! Every bindable action, its default keys, and the lookup from a key press.
//!
//! A binding (`Chord`) is either a direct key (`ctrl+right`) or a key pressed
//! after the prefix (`prefix x`). Plain printable keys are never accepted as
//! direct global bindings, since they would steal typing from the shell.

use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

const MODS: KeyModifiers =
    KeyModifiers::CONTROL.union(KeyModifiers::ALT).union(KeyModifiers::SHIFT);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct KeyCombo {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

impl KeyCombo {
    /// Canonical form, so a key press and its configured spelling compare equal:
    /// shift is already part of a character, `shift+tab` is `backtab`, and
    /// control letters are lower case.
    pub fn new(code: KeyCode, mods: KeyModifiers) -> Self {
        let mut mods = mods & MODS;
        let mut code = code;
        match code {
            KeyCode::Char(c) => {
                if mods.contains(KeyModifiers::CONTROL) {
                    code = KeyCode::Char(c.to_ascii_lowercase());
                }
                mods.remove(KeyModifiers::SHIFT);
            }
            KeyCode::Tab if mods.contains(KeyModifiers::SHIFT) => {
                code = KeyCode::BackTab;
                mods.remove(KeyModifiers::SHIFT);
            }
            KeyCode::BackTab => mods.remove(KeyModifiers::SHIFT),
            _ => {}
        }
        Self { code, mods }
    }

    pub fn from_event(key: KeyEvent) -> Self {
        Self::new(key.code, key.modifiers)
    }

    /// A printable key with no ctrl/alt: typing, not a command.
    fn is_plain_text(self) -> bool {
        matches!(self.code, KeyCode::Char(_))
            && !self.mods.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let (mod_part, key_part) = if text == "+" {
            ("", "+")
        } else if let Some(head) = text.strip_suffix("++") {
            (head, "+")
        } else {
            text.rsplit_once('+').unwrap_or(("", text))
        };

        let mut mods = KeyModifiers::NONE;
        for m in mod_part.split('+').filter(|m| !m.is_empty()) {
            mods |= match m.to_lowercase().as_str() {
                "ctrl" | "control" => KeyModifiers::CONTROL,
                "alt" | "meta" | "option" => KeyModifiers::ALT,
                "shift" => KeyModifiers::SHIFT,
                other => return Err(format!("unknown modifier '{other}'")),
            };
        }

        let lower = key_part.to_lowercase();
        let mut code = match lower.as_str() {
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "tab" => KeyCode::Tab,
            "backtab" => KeyCode::BackTab,
            "enter" | "return" => KeyCode::Enter,
            "esc" | "escape" => KeyCode::Esc,
            "space" => KeyCode::Char(' '),
            "backspace" => KeyCode::Backspace,
            "delete" | "del" => KeyCode::Delete,
            "insert" => KeyCode::Insert,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" => KeyCode::PageUp,
            "pagedown" => KeyCode::PageDown,
            f if f.len() > 1
                && f.starts_with('f')
                && f[1..].chars().all(|c| c.is_ascii_digit()) =>
            {
                match f[1..].parse::<u8>() {
                    Ok(n @ 1..=12) => KeyCode::F(n),
                    _ => return Err(format!("unknown key '{key_part}'")),
                }
            }
            _ => {
                let mut chars = key_part.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => KeyCode::Char(c),
                    _ => return Err(format!("unknown key '{key_part}'")),
                }
            }
        };
        if let (KeyCode::Char(c), true) = (code, mods.contains(KeyModifiers::SHIFT)) {
            code = KeyCode::Char(c.to_ascii_uppercase());
        }
        Ok(Self::new(code, mods))
    }

    pub fn format(self) -> String {
        let mut out = String::new();
        for (flag, name) in [
            (KeyModifiers::CONTROL, "ctrl+"),
            (KeyModifiers::ALT, "alt+"),
            (KeyModifiers::SHIFT, "shift+"),
        ] {
            if self.mods.contains(flag) {
                out.push_str(name);
            }
        }
        match self.code {
            KeyCode::Char(' ') => out.push_str("space"),
            KeyCode::Char(c) => out.push(c),
            KeyCode::F(n) => out.push_str(&format!("f{n}")),
            KeyCode::Left => out.push_str("left"),
            KeyCode::Right => out.push_str("right"),
            KeyCode::Up => out.push_str("up"),
            KeyCode::Down => out.push_str("down"),
            KeyCode::Tab => out.push_str("tab"),
            KeyCode::BackTab => out.push_str("backtab"),
            KeyCode::Enter => out.push_str("enter"),
            KeyCode::Esc => out.push_str("esc"),
            KeyCode::Backspace => out.push_str("backspace"),
            KeyCode::Delete => out.push_str("delete"),
            KeyCode::Insert => out.push_str("insert"),
            KeyCode::Home => out.push_str("home"),
            KeyCode::End => out.push_str("end"),
            KeyCode::PageUp => out.push_str("pageup"),
            KeyCode::PageDown => out.push_str("pagedown"),
            other => out.push_str(&format!("{other:?}").to_lowercase()),
        }
        out
    }
}

/// One binding: a key, optionally pressed after the prefix.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Chord {
    pub prefix: bool,
    pub key: KeyCombo,
}

impl Chord {
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        match text.strip_prefix("prefix ").or_else(|| text.strip_prefix("prefix\t")) {
            Some(rest) => Ok(Self { prefix: true, key: KeyCombo::parse(rest)? }),
            None => Ok(Self { prefix: false, key: KeyCombo::parse(text)? }),
        }
    }

    pub fn format(self) -> String {
        if self.prefix {
            format!("prefix {}", self.key.format())
        } else {
            self.key.format()
        }
    }
}

/// Where an action applies. Global actions work everywhere; the others only
/// when a pane of that kind has focus.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scope {
    Global,
    Files,
    Editor,
}

impl Scope {
    pub fn tag(self) -> &'static str {
        match self {
            Scope::Global => "all",
            Scope::Files => "files",
            Scope::Editor => "editor",
        }
    }

    fn overlaps(self, other: Scope) -> bool {
        self == Scope::Global || other == Scope::Global || self == other
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Action {
    SplitLeft,
    SplitRight,
    SplitDown,
    NewTerminal,
    OpenGraph,
    OpenFiles,
    ClosePane,
    ZoomPane,
    NextPane,
    PrevPane,
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    ResizeLeft,
    ResizeRight,
    ResizeUp,
    ResizeDown,
    Detach,
    QuitSession,
    OpenMenu,
    OpenKeybinds,
    OpenSettings,
    OpenThemes,
    NewWorkspace,
    CloseWorkspace,
    RenameWorkspace,
    NextWorkspace,
    PrevWorkspace,
    /// Jump to the n-th workspace (1-based).
    GotoWorkspace(u8),
    /// Jump to the n-th pane of the current workspace (1-based, layout order).
    GotoPane(u8),
    FileNewFile,
    FileNewDir,
    FileRename,
    FileMove,
    FileDelete,
    FileCopy,
    FileCut,
    FilePaste,
    FileEdit,
    FileToggleHidden,
    FileRefresh,
    EditorSave,
    EditorStartEdit,
    EditorStopEdit,
    EditorReload,
}

use Action::*;

impl Action {
    /// Every action, in the order the keybinding dialog lists them.
    pub fn all() -> Vec<Action> {
        let mut all = vec![
            SplitLeft,
            SplitRight,
            SplitDown,
            NewTerminal,
            OpenGraph,
            OpenFiles,
            ClosePane,
            ZoomPane,
            NextPane,
            PrevPane,
            FocusLeft,
            FocusRight,
            FocusUp,
            FocusDown,
            ResizeLeft,
            ResizeRight,
            ResizeUp,
            ResizeDown,
            NewWorkspace,
            RenameWorkspace,
            CloseWorkspace,
            NextWorkspace,
            PrevWorkspace,
        ];
        all.extend((1..=9).map(GotoWorkspace));
        all.extend((1..=9).map(GotoPane));
        all.extend([
            Detach,
            QuitSession,
            OpenMenu,
            OpenKeybinds,
            OpenSettings,
            OpenThemes,
            FileNewFile,
            FileNewDir,
            FileRename,
            FileMove,
            FileDelete,
            FileCopy,
            FileCut,
            FilePaste,
            FileEdit,
            FileToggleHidden,
            FileRefresh,
            EditorSave,
            EditorStartEdit,
            EditorStopEdit,
            EditorReload,
        ]);
        all
    }

    pub fn scope(self) -> Scope {
        match self {
            FileNewFile | FileNewDir | FileRename | FileMove | FileDelete | FileCopy | FileCut
            | FilePaste | FileEdit | FileToggleHidden | FileRefresh => Scope::Files,
            EditorSave | EditorStartEdit | EditorStopEdit | EditorReload => Scope::Editor,
            _ => Scope::Global,
        }
    }

    /// The key used for this action in the config file.
    pub fn name(self) -> String {
        let s = match self {
            SplitLeft => "split_left",
            SplitRight => "split_right",
            SplitDown => "split_down",
            NewTerminal => "new_terminal",
            OpenGraph => "open_graph",
            OpenFiles => "open_files",
            ClosePane => "close_pane",
            ZoomPane => "zoom_pane",
            NextPane => "next_pane",
            PrevPane => "prev_pane",
            FocusLeft => "focus_left",
            FocusRight => "focus_right",
            FocusUp => "focus_up",
            FocusDown => "focus_down",
            ResizeLeft => "resize_left",
            ResizeRight => "resize_right",
            ResizeUp => "resize_up",
            ResizeDown => "resize_down",
            Detach => "detach",
            QuitSession => "quit_session",
            OpenMenu => "open_menu",
            OpenKeybinds => "open_keybinds",
            OpenSettings => "open_settings",
            OpenThemes => "open_themes",
            NewWorkspace => "new_workspace",
            CloseWorkspace => "close_workspace",
            RenameWorkspace => "rename_workspace",
            NextWorkspace => "next_workspace",
            PrevWorkspace => "prev_workspace",
            GotoWorkspace(n) => return format!("goto_workspace_{n}"),
            GotoPane(n) => return format!("goto_pane_{n}"),
            FileNewFile => "file_new_file",
            FileNewDir => "file_new_dir",
            FileRename => "file_rename",
            FileMove => "file_move",
            FileDelete => "file_delete",
            FileCopy => "file_copy",
            FileCut => "file_cut",
            FilePaste => "file_paste",
            FileEdit => "file_edit",
            FileToggleHidden => "file_toggle_hidden",
            FileRefresh => "file_refresh",
            EditorSave => "editor_save",
            EditorStartEdit => "editor_start_edit",
            EditorStopEdit => "editor_stop_edit",
            EditorReload => "editor_reload",
        };
        s.to_string()
    }

    pub fn from_name(name: &str) -> Option<Action> {
        Action::all().into_iter().find(|a| a.name() == name)
    }

    pub fn label(self) -> String {
        match self {
            SplitLeft => "Split pane to the left".into(),
            SplitRight => "Split pane to the right".into(),
            SplitDown => "Split pane downwards".into(),
            NewTerminal => "New terminal".into(),
            OpenGraph => "Open git graph".into(),
            OpenFiles => "Open file browser".into(),
            ClosePane => "Close pane".into(),
            ZoomPane => "Zoom / unzoom pane".into(),
            NextPane => "Next pane".into(),
            PrevPane => "Previous pane".into(),
            FocusLeft => "Focus pane to the left".into(),
            FocusRight => "Focus pane to the right".into(),
            FocusUp => "Focus pane above".into(),
            FocusDown => "Focus pane below".into(),
            ResizeLeft => "Resize: move divider left".into(),
            ResizeRight => "Resize: move divider right".into(),
            ResizeUp => "Resize: move divider up".into(),
            ResizeDown => "Resize: move divider down".into(),
            Detach => "Detach from session".into(),
            QuitSession => "Quit session (kills shells)".into(),
            OpenMenu => "Open the menu".into(),
            OpenKeybinds => "Open keybindings".into(),
            OpenSettings => "Open settings".into(),
            OpenThemes => "Open theme picker".into(),
            NewWorkspace => "New workspace".into(),
            CloseWorkspace => "Close workspace (and its panes)".into(),
            RenameWorkspace => "Rename workspace".into(),
            NextWorkspace => "Next workspace".into(),
            PrevWorkspace => "Previous workspace".into(),
            GotoWorkspace(n) => format!("Jump to workspace {n}"),
            GotoPane(n) => format!("Jump to pane {n}"),
            FileNewFile => "Files: new file".into(),
            FileNewDir => "Files: new folder".into(),
            FileRename => "Files: rename".into(),
            FileMove => "Files: move".into(),
            FileDelete => "Files: delete".into(),
            FileCopy => "Files: copy".into(),
            FileCut => "Files: cut".into(),
            FilePaste => "Files: paste".into(),
            FileEdit => "Files: edit file".into(),
            FileToggleHidden => "Files: show/hide hidden".into(),
            FileRefresh => "Files: refresh".into(),
            EditorSave => "Editor: save".into(),
            EditorStartEdit => "Editor: start editing".into(),
            EditorStopEdit => "Editor: stop editing".into(),
            EditorReload => "Editor: reload from disk".into(),
        }
    }

    /// Short word for the status-line hints.
    pub fn short(self) -> String {
        match self {
            SplitLeft => "split←".into(),
            SplitRight => "split→".into(),
            SplitDown => "split↓".into(),
            NewTerminal => "new".into(),
            OpenGraph => "graph".into(),
            OpenFiles => "files".into(),
            ClosePane => "close".into(),
            ZoomPane => "zoom".into(),
            NextPane => "next".into(),
            PrevPane => "prev".into(),
            FocusLeft | FocusRight | FocusUp | FocusDown => "focus".into(),
            ResizeLeft | ResizeRight | ResizeUp | ResizeDown => "resize".into(),
            Detach => "detach".into(),
            QuitSession => "quit".into(),
            OpenMenu => "menu".into(),
            OpenKeybinds => "keys".into(),
            OpenSettings => "settings".into(),
            OpenThemes => "theme".into(),
            NewWorkspace => "new space".into(),
            CloseWorkspace => "close space".into(),
            RenameWorkspace => "rename space".into(),
            NextWorkspace => "next space".into(),
            PrevWorkspace => "prev space".into(),
            GotoWorkspace(n) => format!("space {n}"),
            GotoPane(n) => format!("pane {n}"),
            FileNewFile => "new file".into(),
            FileNewDir => "new folder".into(),
            FileRename => "rename".into(),
            FileMove => "move".into(),
            FileDelete => "delete".into(),
            FileCopy => "copy".into(),
            FileCut => "cut".into(),
            FilePaste => "paste".into(),
            FileEdit => "edit".into(),
            FileToggleHidden => "hidden".into(),
            FileRefresh => "refresh".into(),
            EditorSave => "save".into(),
            EditorStartEdit => "edit".into(),
            EditorStopEdit => "stop editing".into(),
            EditorReload => "reload".into(),
        }
    }

    pub fn defaults(self) -> Vec<String> {
        let list: &[&str] = match self {
            SplitLeft => &["ctrl+left"],
            SplitRight => &["ctrl+right", "prefix %", "prefix |", "prefix v"],
            SplitDown => &["ctrl+down", "prefix \"", "prefix -", "prefix s"],
            NewTerminal => &["prefix c"],
            OpenGraph => &["prefix g"],
            OpenFiles => &["prefix f"],
            ClosePane => &["prefix x"],
            ZoomPane => &["prefix z"],
            NextPane => &["alt+pagedown", "ctrl+tab", "prefix o", "prefix tab"],
            PrevPane => &["alt+pageup", "ctrl+backtab", "prefix O", "prefix backtab"],
            FocusLeft => &["alt+left", "prefix h", "prefix left"],
            FocusRight => &["alt+right", "prefix l", "prefix right"],
            FocusUp => &["alt+up", "prefix k", "prefix up"],
            FocusDown => &["alt+down", "prefix j", "prefix down"],
            ResizeLeft => &["prefix H"],
            ResizeRight => &["prefix L"],
            ResizeUp => &["prefix K"],
            ResizeDown => &["prefix J"],
            Detach => &["prefix d"],
            QuitSession => &["prefix q"],
            OpenMenu => &["prefix m"],
            OpenKeybinds => &["prefix ?"],
            OpenSettings => &["prefix ,"],
            OpenThemes => &["prefix t"],
            NewWorkspace => &["prefix N"],
            CloseWorkspace => &["prefix X"],
            RenameWorkspace => &["prefix r"],
            NextWorkspace => &["prefix n"],
            PrevWorkspace => &["prefix p"],
            GotoWorkspace(n) => return vec![format!("ctrl+{n}"), format!("prefix {n}")],
            GotoPane(n) => return vec![format!("alt+{n}")],
            FileNewFile => &["n"],
            FileNewDir => &["N"],
            FileRename => &["r", "f2"],
            FileMove => &["m"],
            FileDelete => &["d", "delete"],
            FileCopy => &["c", "y"],
            FileCut => &["x"],
            FilePaste => &["p"],
            FileEdit => &["e"],
            FileToggleHidden => &["."],
            FileRefresh => &["R", "f5"],
            EditorSave => &["ctrl+s"],
            EditorStartEdit => &["e", "i", "enter"],
            EditorStopEdit => &["esc"],
            EditorReload => &["r"],
        };
        list.iter().map(|s| s.to_string()).collect()
    }
}

fn parse_all(list: &[String]) -> Vec<Chord> {
    list.iter().filter_map(|s| Chord::parse(s).ok()).collect()
}

#[derive(Clone, PartialEq)]
pub struct Keymap {
    pub prefix: KeyCombo,
    binds: HashMap<Action, Vec<Chord>>,
}

const DEFAULT_PREFIX: &str = "ctrl+b";

impl Default for Keymap {
    fn default() -> Self {
        let binds = Action::all().into_iter().map(|a| (a, parse_all(&a.defaults()))).collect();
        Self { prefix: KeyCombo::parse(DEFAULT_PREFIX).expect("the default prefix parses"), binds }
    }
}

impl Keymap {
    pub fn chords(&self, action: Action) -> &[Chord] {
        self.binds.get(&action).map_or(&[], Vec::as_slice)
    }

    pub fn set(&mut self, action: Action, chords: Vec<Chord>) {
        self.binds.insert(action, chords);
    }

    pub fn reset(&mut self, action: Action) {
        self.set(action, parse_all(&action.defaults()));
    }

    /// The first binding of `action`, for hints; `"unbound"` if there is none.
    pub fn hint(&self, action: Action) -> String {
        self.chords(action).first().map_or_else(|| "unbound".to_string(), |c| c.format())
    }

    /// The action for a key press. `scope` is the focused pane's own scope;
    /// global actions are always considered first.
    pub fn lookup(&self, prefixed: bool, key: KeyCombo, scope: Scope) -> Option<Action> {
        let wanted = Chord { prefix: prefixed, key };
        Action::all().into_iter().find(|a| {
            (a.scope() == Scope::Global || a.scope() == scope) && self.chords(*a).contains(&wanted)
        })
    }

    /// Another action that already uses `chord` where `action` would also apply.
    pub fn conflict(&self, chord: Chord, action: Action) -> Option<Action> {
        Action::all().into_iter().find(|other| {
            *other != action
                && other.scope().overlaps(action.scope())
                && self.chords(*other).contains(&chord)
        })
    }

    /// Why `chord` cannot be bound to `action`, if it cannot.
    pub fn check(&self, chord: Chord, action: Action) -> Result<(), String> {
        if chord.prefix && chord.key == self.prefix {
            return Err(
                "that is the prefix key itself (prefix + prefix sends it to the shell)".into()
            );
        }
        if !chord.prefix && chord.key == self.prefix {
            return Err("that key is the prefix key".into());
        }
        if !chord.prefix && action.scope() == Scope::Global && chord.key.is_plain_text() {
            return Err(
                "a plain key would block typing in terminals; add ctrl/alt or use prefix".into()
            );
        }
        if let Some(other) = self.conflict(chord, action) {
            return Err(format!("already used by '{}'", other.label()));
        }
        Ok(())
    }

    /// Whether `key` is used by any direct (non-prefix) binding.
    pub fn uses_direct(&self, key: KeyCombo) -> Option<Action> {
        let wanted = Chord { prefix: false, key };
        Action::all().into_iter().find(|a| self.chords(*a).contains(&wanted))
    }

    /// Apply the `[keys]` table of a config file over the defaults; returns warnings.
    pub fn apply(&mut self, table: &toml::Table) -> Vec<String> {
        let mut warnings = Vec::new();
        for (name, value) in table {
            if name == "prefix" {
                match value.as_str().map(KeyCombo::parse) {
                    Some(Ok(k)) => self.prefix = k,
                    Some(Err(e)) => warnings.push(format!("keys.prefix: {e}")),
                    None => warnings.push("keys.prefix must be a string".into()),
                }
                continue;
            }
            let Some(action) = Action::from_name(name) else {
                warnings.push(format!("unknown action '{name}' in [keys]"));
                continue;
            };
            let strings: Vec<&str> = match value {
                toml::Value::String(s) => vec![s.as_str()],
                toml::Value::Array(a) => a.iter().filter_map(toml::Value::as_str).collect(),
                _ => {
                    warnings.push(format!("keys.{name} must be a string or a list of strings"));
                    continue;
                }
            };
            let mut chords = Vec::new();
            for s in strings {
                match Chord::parse(s) {
                    Ok(c) => chords.push(c),
                    Err(e) => warnings.push(format!("keys.{name}: '{s}': {e}")),
                }
            }
            self.set(action, chords);
        }
        warnings
    }

    /// Only what differs from the defaults, ready to write to `[keys]`.
    pub fn overrides(&self) -> toml::Table {
        let default = Keymap::default();
        let mut table = toml::Table::new();
        if self.prefix != default.prefix {
            table.insert("prefix".into(), toml::Value::String(self.prefix.format()));
        }
        for action in Action::all() {
            if self.chords(action) != default.chords(action) {
                let list = self.chords(action).iter().map(|c| toml::Value::String(c.format()));
                table.insert(action.name(), toml::Value::Array(list.collect()));
            }
        }
        table
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn combo(s: &str) -> KeyCombo {
        KeyCombo::parse(s).unwrap()
    }

    #[test]
    fn parses_and_formats() {
        for s in [
            "ctrl+right",
            "ctrl+1",
            "f5",
            "esc",
            "enter",
            "space",
            "x",
            "H",
            "%",
            "ctrl+alt+left",
            "+",
            "ctrl++",
        ] {
            assert_eq!(combo(s).format(), s, "{s}");
        }
        assert_eq!(combo("shift+tab"), combo("backtab"));
        assert_eq!(combo("shift+h"), combo("H"));
        assert_eq!(combo("CTRL+Right"), combo("ctrl+right"));
        assert!(KeyCombo::parse("ctrl+").is_err());
        assert!(KeyCombo::parse("hyper+x").is_err());
        assert!(KeyCombo::parse("f13").is_err());
    }

    #[test]
    fn key_events_normalise_to_configured_spelling() {
        let ev = |code, mods| KeyCombo::new(code, mods);
        assert_eq!(ev(KeyCode::Char('H'), KeyModifiers::SHIFT), combo("H"));
        assert_eq!(
            ev(KeyCode::Char('B'), KeyModifiers::CONTROL | KeyModifiers::SHIFT),
            combo("ctrl+b")
        );
        assert_eq!(ev(KeyCode::Tab, KeyModifiers::SHIFT), combo("backtab"));
        assert_eq!(ev(KeyCode::BackTab, KeyModifiers::SHIFT), combo("backtab"));
        assert_eq!(ev(KeyCode::Right, KeyModifiers::CONTROL), combo("ctrl+right"));
    }

    #[test]
    fn chords_round_trip() {
        let c = Chord::parse("prefix %").unwrap();
        assert!(c.prefix);
        assert_eq!(c.format(), "prefix %");
        assert!(!Chord::parse("ctrl+left").unwrap().prefix);
    }

    #[test]
    fn every_default_parses_and_has_no_conflicts() {
        let map = Keymap::default();
        for a in Action::all() {
            assert_eq!(
                map.chords(a).len(),
                a.defaults().len(),
                "{} has an unparsable default",
                a.name()
            );
            for c in map.chords(a) {
                assert_eq!(map.conflict(*c, a), None, "{} conflicts on {}", a.name(), c.format());
            }
            assert_eq!(Action::from_name(&a.name()), Some(a));
        }
    }

    #[test]
    fn default_bindings_match_the_spec() {
        let map = Keymap::default();
        let g = |p, k: &str| map.lookup(p, combo(k), Scope::Global);
        assert_eq!(g(false, "ctrl+left"), Some(SplitLeft));
        assert_eq!(g(false, "ctrl+right"), Some(SplitRight));
        assert_eq!(g(false, "ctrl+down"), Some(SplitDown));
        assert_eq!(g(false, "ctrl+up"), None, "there is no split-up");
        assert_eq!(g(false, "ctrl+tab"), Some(NextPane));
        assert_eq!(g(false, "ctrl+shift+tab"), Some(PrevPane));
        assert_eq!(g(false, "alt+3"), Some(GotoPane(3)), "alt+digit jumps to a pane");
        assert_eq!(g(false, "ctrl+3"), Some(GotoWorkspace(3)), "ctrl+digit jumps to a workspace");
        // Keys GNOME Terminal and similar emulators pass through to programs.
        assert_eq!(g(false, "alt+left"), Some(FocusLeft));
        assert_eq!(g(false, "alt+down"), Some(FocusDown));
        assert_eq!(g(false, "alt+pagedown"), Some(NextPane));
        assert_eq!(g(false, "alt+pageup"), Some(PrevPane));
        assert_eq!(g(true, "3"), Some(GotoWorkspace(3)), "prefix+digit is the workspace fallback");
        assert_eq!(g(true, "x"), Some(ClosePane));
    }

    #[test]
    fn scoped_lookup_only_applies_in_its_pane() {
        let map = Keymap::default();
        assert_eq!(map.lookup(false, combo("n"), Scope::Files), Some(FileNewFile));
        assert_eq!(map.lookup(false, combo("n"), Scope::Global), None);
        assert_eq!(map.lookup(false, combo("n"), Scope::Editor), None);
        assert_eq!(map.lookup(false, combo("ctrl+s"), Scope::Editor), Some(EditorSave));
        // The same key may mean different things in different scopes.
        assert_eq!(map.lookup(false, combo("r"), Scope::Files), Some(FileRename));
        assert_eq!(map.lookup(false, combo("r"), Scope::Editor), Some(EditorReload));
    }

    #[test]
    fn check_rejects_unsafe_and_conflicting_bindings() {
        let map = Keymap::default();
        let chord = |s| Chord::parse(s).unwrap();
        assert!(map.check(chord("ctrl+right"), ClosePane).unwrap_err().contains("already used"));
        assert!(map.check(chord("q"), ClosePane).unwrap_err().contains("block typing"));
        assert!(map.check(chord("ctrl+b"), ClosePane).is_err());
        assert!(map.check(chord("prefix ctrl+b"), ClosePane).is_err());
        assert!(map.check(chord("prefix w"), ClosePane).is_ok());
        assert!(map.check(chord("alt+w"), ClosePane).is_ok());
        assert!(
            map.check(chord("q"), FileDelete).is_ok(),
            "plain keys are fine inside the files pane"
        );
        assert!(
            map.check(chord("n"), EditorSave).is_ok(),
            "files and editor scopes do not overlap"
        );
        assert!(map.check(chord("n"), FileRename).unwrap_err().contains("already used"));
    }

    #[test]
    fn overrides_round_trip_and_only_store_differences() {
        let mut map = Keymap::default();
        assert!(map.overrides().is_empty());

        map.set(ClosePane, vec![Chord::parse("alt+w").unwrap()]);
        map.set(SplitLeft, vec![]);
        map.prefix = combo("ctrl+a");
        let table = map.overrides();
        assert_eq!(table.len(), 3);

        let mut fresh = Keymap::default();
        assert!(fresh.apply(&table).is_empty());
        assert!(fresh == map);
        assert_eq!(
            fresh.lookup(false, combo("ctrl+left"), Scope::Global),
            None,
            "unbound stays unbound"
        );
    }

    #[test]
    fn apply_reports_problems_but_keeps_going() {
        let table: toml::Table = toml::from_str(
            "close_pane = \"alt+w\"\nbogus = \"x\"\nzoom_pane = [\"prefix z\", \"hyper+q\"]\nprefix = 5\n",
        )
        .unwrap();
        let mut map = Keymap::default();
        let warnings = map.apply(&table);
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        assert_eq!(map.lookup(false, combo("alt+w"), Scope::Global), Some(ClosePane));
        assert_eq!(map.chords(ZoomPane).len(), 1);
    }
}
