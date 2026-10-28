//! Modal dialogs: keybindings, settings, and the theme picker. While one is
//! open it receives every key press.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph, Wrap},
    Frame,
};

use crate::{
    app::App,
    keymap::{Action, Chord, KeyCombo, Keymap},
    layout::PaneId,
    settings_dialog::SettingsDialog,
    theme::{self, Theme},
};

pub enum Overlay {
    Keybinds(Keybinds),
    Settings(SettingsDialog),
    Themes(ThemePicker),
    Name(NameInput),
    Confirm(Confirm),
}

impl Overlay {
    /// Handle a key; returns false when the dialog should close.
    pub fn key(&mut self, app: &mut App, key: KeyEvent) -> bool {
        match self {
            Overlay::Keybinds(d) => d.key(app, key),
            Overlay::Settings(d) => d.key(app, key),
            Overlay::Themes(d) => d.key(app, key),
            Overlay::Name(d) => d.key(app, key),
            Overlay::Confirm(d) => d.key(app, key),
        }
    }

    /// Called every server tick so a dialog can pick up background results.
    pub fn poll(&mut self) {
        if let Overlay::Settings(d) = self {
            d.poll();
        }
    }

    /// A left click at a screen position; returns false when the dialog should close.
    pub fn click(&mut self, app: &mut App, x: u16, y: u16) -> bool {
        match self {
            Overlay::Confirm(d) => d.click(app, x, y),
            _ => true,
        }
    }

    /// Mouse wheel scrolling; positive is towards the end.
    pub fn scroll(&mut self, delta: isize) {
        match self {
            Overlay::Keybinds(d) => d.step(delta),
            Overlay::Settings(d) => d.scroll_by(delta),
            Overlay::Themes(_) | Overlay::Name(_) | Overlay::Confirm(_) => {}
        }
    }
}

/// A box centred in the screen, leaving the dock and status rows visible.
pub(crate) fn centered(area: Rect, max_w: u16, max_h: u16) -> Rect {
    let usable = area.height.saturating_sub(2);
    let w = area.width.saturating_sub(2).min(max_w);
    let h = usable.saturating_sub(2).min(max_h);
    Rect::new(area.x + (area.width - w) / 2, area.y + (usable - h) / 2, w, h)
}

pub(crate) fn frame(f: &mut Frame, area: Rect, title: &str, th: &Theme) -> Rect {
    let block =
        Block::bordered().title(format!(" {title} ")).border_style(Style::new().fg(th.accent));
    let inner = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    inner
}

pub(crate) fn reversed() -> Style {
    Style::new().add_modifier(Modifier::REVERSED)
}

/// Keep `selected` inside the window starting at `scroll`.
pub(crate) fn keep_visible(scroll: &mut usize, selected: usize, height: usize) {
    if selected < *scroll {
        *scroll = selected;
    } else if height > 0 && selected >= *scroll + height {
        *scroll = selected + 1 - height;
    }
}

// ---- keybindings -------------------------------------------------------------

#[derive(Clone, Copy)]
enum Row {
    Prefix,
    Action(Action),
}

#[derive(Clone, Copy)]
enum Capture {
    Replace,
    Add,
    AddPrefixed,
}

pub struct Keybinds {
    rows: Vec<Row>,
    selected: usize,
    scroll: usize,
    capture: Option<Capture>,
    message: Option<String>,
    /// The last key press that reached the app, to diagnose keys a terminal swallows.
    last_key: Option<String>,
}

impl Keybinds {
    pub fn new() -> Self {
        let mut rows = vec![Row::Prefix];
        rows.extend(Action::all().into_iter().map(Row::Action));
        Self { rows, selected: 0, scroll: 0, capture: None, message: None, last_key: None }
    }

    fn step(&mut self, delta: isize) {
        self.selected =
            (self.selected as isize + delta).clamp(0, self.rows.len() as isize - 1) as usize;
    }

    fn key(&mut self, app: &mut App, key: KeyEvent) -> bool {
        self.last_key = Some(KeyCombo::from_event(key).format());
        if let Some(mode) = self.capture.take() {
            if key.code == KeyCode::Esc {
                self.message = Some("cancelled".into());
            } else {
                self.message = Some(self.bind(app, mode, KeyCombo::from_event(key)));
            }
            return true;
        }
        self.message = None;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return false,
            KeyCode::Up | KeyCode::Char('k') => self.step(-1),
            KeyCode::Down | KeyCode::Char('j') => self.step(1),
            KeyCode::PageUp => self.step(-10),
            KeyCode::PageDown => self.step(10),
            KeyCode::Home | KeyCode::Char('g') => self.selected = 0,
            KeyCode::End | KeyCode::Char('G') => self.selected = self.rows.len() - 1,
            KeyCode::Enter | KeyCode::Char('r') => self.capture = Some(Capture::Replace),
            KeyCode::Char('a') => self.capture = Some(Capture::Add),
            KeyCode::Char('p') => {
                if matches!(self.rows[self.selected], Row::Prefix) {
                    self.message = Some("the prefix key itself is always a direct key".into());
                } else {
                    self.capture = Some(Capture::AddPrefixed);
                }
            }
            KeyCode::Char('d') | KeyCode::Delete | KeyCode::Backspace => self.clear(app),
            KeyCode::Char('u') => self.reset(app, false),
            KeyCode::Char('U') => self.reset(app, true),
            _ => {}
        }
        true
    }

    fn bind(&mut self, app: &mut App, mode: Capture, combo: KeyCombo) -> String {
        match self.rows[self.selected] {
            Row::Prefix => {
                let plain = matches!(combo.code, KeyCode::Char(_))
                    && !combo.mods.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
                if plain {
                    return "a plain key would block typing; use ctrl or alt".into();
                }
                if let Some(a) = app.config.keymap.uses_direct(combo) {
                    return format!("already used by '{}'", a.label());
                }
                app.config.keymap.prefix = combo;
                format!("prefix is now {}; {}", combo.format(), app.save_config())
            }
            Row::Action(action) => {
                let chord = Chord { prefix: matches!(mode, Capture::AddPrefixed), key: combo };
                if let Err(why) = app.config.keymap.check(chord, action) {
                    return format!("{} not bound: {why}", chord.format());
                }
                let mut list = match mode {
                    Capture::Replace => Vec::new(),
                    _ => app.config.keymap.chords(action).to_vec(),
                };
                if !list.contains(&chord) {
                    list.push(chord);
                }
                app.config.keymap.set(action, list);
                format!("bound {}; {}", chord.format(), app.save_config())
            }
        }
    }

    fn clear(&mut self, app: &mut App) {
        self.message = Some(match self.rows[self.selected] {
            Row::Prefix => "the prefix key cannot be cleared".into(),
            Row::Action(a) => {
                app.config.keymap.set(a, Vec::new());
                format!("cleared; {}", app.save_config())
            }
        });
    }

    fn reset(&mut self, app: &mut App, all: bool) {
        let default = Keymap::default();
        if all {
            app.config.keymap = default;
        } else {
            match self.rows[self.selected] {
                Row::Prefix => app.config.keymap.prefix = default.prefix,
                Row::Action(a) => app.config.keymap.reset(a),
            }
        }
        self.message = Some(format!("reset to default; {}", app.save_config()));
    }
}

pub fn draw_keybinds(f: &mut Frame, d: &mut Keybinds, th: &Theme, keymap: &Keymap) {
    let area = centered(f.area(), 100, 40);
    let inner = frame(f, area, "Keybindings", th);
    let [list, footer] = Layout::vertical([Constraint::Min(1), Constraint::Length(3)]).areas(inner);
    keep_visible(&mut d.scroll, d.selected, list.height as usize);

    let lines: Vec<Line> = d
        .rows
        .iter()
        .enumerate()
        .skip(d.scroll)
        .take(list.height as usize)
        .map(|(i, row)| {
            let (label, scope, keys) = match row {
                Row::Prefix => ("Prefix key".to_string(), "all", keymap.prefix.format()),
                Row::Action(a) => {
                    let chords = keymap.chords(*a);
                    let keys = if chords.is_empty() {
                        "(unbound)".to_string()
                    } else {
                        chords.iter().map(|c| c.format()).collect::<Vec<_>>().join(", ")
                    };
                    (a.label(), a.scope().tag(), keys)
                }
            };
            let style = if i == d.selected { reversed() } else { Style::new() };
            Line::from(vec![
                Span::styled(format!(" {label:<32}"), style),
                Span::styled(
                    format!("{scope:<8}"),
                    if i == d.selected { style } else { Style::new().fg(th.dim) },
                ),
                Span::styled(keys, style),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lines), list);

    let status = match (&d.capture, &d.message) {
        (Some(_), _) => Line::from(Span::styled(
            " Press the new key now (Esc cancels)...",
            Style::new().fg(th.marker).bold(),
        )),
        (None, Some(m)) => Line::from(Span::styled(format!(" {m}"), Style::new().fg(th.marker))),
        (None, None) => Line::from(Span::styled(
            match &d.last_key {
                Some(k) => format!(
                    " last key received: {k}  (a key that never shows up here is intercepted by your terminal)"
                ),
                None => format!(
                    " prefix = {}; press any key to see what your terminal sends",
                    keymap.prefix.format()
                ),
            },
            Style::new().fg(th.dim),
        )),
    };
    let hints = Line::from(Span::styled(
        " Enter replace · a add · p add after prefix · d clear · u reset · U reset all · Esc close",
        Style::new().fg(th.dim),
    ));
    f.render_widget(
        Paragraph::new(vec![Line::raw(""), status, hints]).wrap(Wrap { trim: true }),
        footer,
    );
}

// ---- themes ------------------------------------------------------------------

pub struct ThemePicker {
    selected: usize,
    original: String,
}

impl ThemePicker {
    pub fn new(current: &str) -> Self {
        let selected = theme::NAMES.iter().position(|n| *n == current).unwrap_or(0);
        Self { selected, original: current.to_string() }
    }

    fn key(&mut self, app: &mut App, key: KeyEvent) -> bool {
        let n = theme::NAMES.len();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.selected = (self.selected + n - 1) % n,
            KeyCode::Down | KeyCode::Char('j') => self.selected = (self.selected + 1) % n,
            KeyCode::Enter => {
                app.set_theme(theme::NAMES[self.selected]);
                app.notice = Some(app.save_config());
                return false;
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                app.set_theme(&self.original);
                return false;
            }
            _ => return true,
        }
        app.set_theme(theme::NAMES[self.selected]);
        true
    }
}

pub fn draw_themes(f: &mut Frame, d: &ThemePicker, th: &Theme) {
    let area = centered(f.area(), 52, theme::NAMES.len() as u16 + 5);
    let inner = frame(f, area, "Theme", th);
    let [list, footer] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner);

    let lines: Vec<Line> = theme::NAMES
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let t = theme::builtin(name).unwrap_or(*th);
            let style = if i == d.selected { reversed() } else { Style::new() };
            let mut spans = vec![Span::styled(format!(" {name:<14}"), style), Span::raw(" ")];
            for color in [t.accent, t.working, t.blocked, t.danger, t.dir, t.graph_ref] {
                spans.push(Span::styled("██ ", Style::new().fg(color)));
            }
            Line::from(spans)
        })
        .collect();
    f.render_widget(Paragraph::new(lines), list);
    f.render_widget(
        Paragraph::new(" Up/Down preview · Enter keep · Esc cancel").style(Style::new().fg(th.dim)),
        footer,
    );
}

/// Draw whichever dialog is open.
pub fn draw(f: &mut Frame, overlay: &mut Overlay, th: &Theme, keymap: &Keymap) {
    match overlay {
        Overlay::Keybinds(d) => draw_keybinds(f, d, th, keymap),
        Overlay::Settings(d) => crate::settings_dialog::draw(f, d, th),
        Overlay::Themes(d) => draw_themes(f, d, th),
        Overlay::Name(d) => draw_name(f, d, th),
        Overlay::Confirm(d) => draw_confirm(f, d, th),
    }
}

// ---- rename workspace --------------------------------------------------------

pub struct NameInput {
    /// Index of the workspace being renamed.
    ws: usize,
    text: String,
    error: Option<String>,
}

impl NameInput {
    pub fn new(ws: usize, current: &str) -> Self {
        Self { ws, text: current.to_string(), error: None }
    }

    fn key(&mut self, app: &mut App, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc => return false,
            KeyCode::Enter => match app.rename_workspace(self.ws, &self.text) {
                Ok(()) => return false,
                Err(e) => self.error = Some(e),
            },
            KeyCode::Backspace => {
                self.text.pop();
                self.error = None;
            }
            KeyCode::Char(c)
                if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.text.push(c);
                self.error = None;
            }
            _ => {}
        }
        true
    }
}

pub fn draw_name(f: &mut Frame, d: &NameInput, th: &Theme) {
    let area = centered(f.area(), 54, 6);
    let inner = frame(f, area, "Rename workspace", th);
    let [input, error, hint] =
        Layout::vertical([Constraint::Length(2), Constraint::Length(1), Constraint::Length(1)])
            .areas(inner);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(format!(" {}", d.text)),
            Span::styled("▏", Style::new().fg(th.marker)),
        ]))
        .wrap(Wrap { trim: false }),
        input,
    );
    if let Some(e) = &d.error {
        f.render_widget(Paragraph::new(format!(" {e}")).style(Style::new().fg(th.danger)), error);
    }
    f.render_widget(
        Paragraph::new(" Enter save · Esc cancel").style(Style::new().fg(th.dim)),
        hint,
    );
}

// ---- confirmation ------------------------------------------------------------

/// What happens when a confirmation is accepted. A workspace is named, not
/// numbered, so a workspace that disappears while the dialog is open cannot
/// make the answer apply to the wrong one.
#[derive(Clone)]
pub enum ConfirmAction {
    ClosePane(PaneId),
    CloseWorkspace(String),
}

/// A yes/no question. "No" is selected first, so Enter alone never destroys anything;
/// `y` answers yes straight away.
pub struct Confirm {
    title: String,
    lines: Vec<String>,
    action: ConfirmAction,
    yes: &'static str,
    yes_selected: bool,
    yes_rect: Rect,
    no_rect: Rect,
}

impl Confirm {
    pub fn new(
        title: impl Into<String>,
        lines: Vec<String>,
        action: ConfirmAction,
        yes: &'static str,
    ) -> Self {
        Self {
            title: title.into(),
            lines,
            action,
            yes,
            yes_selected: false,
            yes_rect: Rect::default(),
            no_rect: Rect::default(),
        }
    }

    #[cfg(test)]
    pub fn title(&self) -> &str {
        &self.title
    }

    #[cfg(test)]
    pub fn text(&self) -> String {
        self.lines.join(" ")
    }

    fn key(&mut self, app: &mut App, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char('y' | 'Y') => app.run_confirmed(self.action.clone()),
            KeyCode::Char('n' | 'N' | 'q') | KeyCode::Esc => {}
            KeyCode::Left
            | KeyCode::Right
            | KeyCode::Tab
            | KeyCode::BackTab
            | KeyCode::Char('h' | 'l') => {
                self.yes_selected = !self.yes_selected;
                return true;
            }
            KeyCode::Enter => {
                if self.yes_selected {
                    app.run_confirmed(self.action.clone());
                }
            }
            _ => return true,
        }
        false
    }

    fn click(&mut self, app: &mut App, x: u16, y: u16) -> bool {
        let at = ratatui::layout::Position::new(x, y);
        if self.yes_rect.contains(at) {
            app.run_confirmed(self.action.clone());
            false
        } else {
            !self.no_rect.contains(at)
        }
    }
}

pub fn draw_confirm(f: &mut Frame, d: &mut Confirm, th: &Theme) {
    let area = centered(f.area(), 66, d.lines.len() as u16 + 6);
    let inner = frame(f, area, &d.title, th);
    let [text, buttons, hint] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1), Constraint::Length(1)])
            .areas(inner);

    let body: Vec<Line> = d.lines.iter().map(|l| Line::from(format!(" {l}"))).collect();
    f.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), text);

    let yes = format!(" {} (y) ", d.yes);
    let no = " Keep it (n) ".to_string();
    d.yes_rect = Rect::new(buttons.x + 2, buttons.y, yes.chars().count() as u16, 1);
    d.no_rect = Rect::new(d.yes_rect.right() + 2, buttons.y, no.chars().count() as u16, 1);
    let chosen = Style::new().fg(th.danger).add_modifier(Modifier::REVERSED | Modifier::BOLD);
    let kept = Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD);
    let idle = Style::new().fg(th.dim);
    f.render_widget(
        Paragraph::new(yes).style(if d.yes_selected { chosen } else { idle }),
        d.yes_rect,
    );
    f.render_widget(Paragraph::new(no).style(if d.yes_selected { idle } else { kept }), d.no_rect);
    f.render_widget(
        Paragraph::new(" y close · n or Esc keep · ←/→ choose · Enter confirm")
            .style(Style::new().fg(th.dim)),
        hint,
    );
}
