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
    integration::{Action as HookAction, Auth},
    keymap::{Action, Chord, KeyCombo, Keymap},
    settings::Snapshot,
    theme::{self, Theme},
};

pub enum Overlay {
    Keybinds(Keybinds),
    Settings(SettingsDialog),
    Themes(ThemePicker),
}

impl Overlay {
    /// Handle a key; returns false when the dialog should close.
    pub fn key(&mut self, app: &mut App, key: KeyEvent) -> bool {
        match self {
            Overlay::Keybinds(d) => d.key(app, key),
            Overlay::Settings(d) => d.key(app, key),
            Overlay::Themes(d) => d.key(app, key),
        }
    }

    /// Mouse wheel scrolling; positive is towards the end.
    pub fn scroll(&mut self, delta: isize) {
        match self {
            Overlay::Keybinds(d) => d.step(delta),
            Overlay::Settings(d) => {
                d.scroll = (d.scroll as isize + delta).max(0) as usize;
                d.follow = false;
            }
            Overlay::Themes(_) => {}
        }
    }
}

/// A box centred in the screen, leaving the dock and status rows visible.
fn centered(area: Rect, max_w: u16, max_h: u16) -> Rect {
    let usable = area.height.saturating_sub(2);
    let w = area.width.saturating_sub(2).min(max_w);
    let h = usable.saturating_sub(2).min(max_h);
    Rect::new(area.x + (area.width - w) / 2, area.y + (usable - h) / 2, w, h)
}

fn frame(f: &mut Frame, area: Rect, title: &str, th: &Theme) -> Rect {
    let block =
        Block::bordered().title(format!(" {title} ")).border_style(Style::new().fg(th.accent));
    let inner = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    inner
}

fn reversed() -> Style {
    Style::new().add_modifier(Modifier::REVERSED)
}

/// Keep `selected` inside the window starting at `scroll`.
fn keep_visible(scroll: &mut usize, selected: usize, height: usize) {
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
}

impl Keybinds {
    pub fn new() -> Self {
        let mut rows = vec![Row::Prefix];
        rows.extend(Action::all().into_iter().map(Row::Action));
        Self { rows, selected: 0, scroll: 0, capture: None, message: None }
    }

    fn step(&mut self, delta: isize) {
        self.selected =
            (self.selected as isize + delta).clamp(0, self.rows.len() as isize - 1) as usize;
    }

    fn key(&mut self, app: &mut App, key: KeyEvent) -> bool {
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
            format!(" prefix = {}; keys outside the prefix are 'direct'", keymap.prefix.format()),
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

// ---- settings ----------------------------------------------------------------

pub struct SettingsDialog {
    snap: Snapshot,
    selected: usize,
    pub scroll: usize,
    follow: bool,
    message: Option<String>,
}

impl SettingsDialog {
    pub fn new(snap: Snapshot) -> Self {
        Self { snap, selected: 0, scroll: 0, follow: true, message: None }
    }

    fn key(&mut self, app: &mut App, key: KeyEvent) -> bool {
        let last = self.snap.agents.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return false,
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
                self.follow = true;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1).min(last);
                self.follow = true;
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(8);
                self.follow = false;
            }
            KeyCode::PageDown => {
                self.scroll += 8;
                self.follow = false;
            }
            KeyCode::Char('r') => {
                self.snap = app.settings_snapshot();
                self.message = Some("refreshed".into());
            }
            KeyCode::Char(c @ ('i' | 'u')) => {
                let agent = self.snap.agents[self.selected].agent;
                let action = if c == 'i' { HookAction::Install } else { HookAction::Uninstall };
                self.message = Some(match agent.apply(action) {
                    Ok(msg) => format!("{}: {msg}", agent.label()),
                    Err(e) => format!("{}: {e:#}", agent.label()),
                });
                self.snap = app.settings_snapshot();
            }
            _ => {}
        }
        true
    }
}

pub fn draw_settings(f: &mut Frame, d: &mut SettingsDialog, th: &Theme) {
    let area = centered(f.area(), 110, 44);
    let inner = frame(f, area, "Settings", th);
    let [body, footer] = Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).areas(inner);

    let heading = |text: &str| {
        Line::from(Span::styled(format!(" {text}"), Style::new().fg(th.accent).bold()))
    };
    let ok = |text: String| Span::styled(text, Style::new().fg(th.working));
    let warn = |text: String| Span::styled(text, Style::new().fg(th.blocked));
    let no = |text: String| Span::styled(text, Style::new().fg(th.dim));
    let bad = |text: String| Span::styled(text, Style::new().fg(th.danger));

    let mut lines: Vec<Line> = Vec::new();
    let mut selected_line = 0;
    lines.push(heading("Agents: hooks and login"));
    for (i, row) in d.snap.agents.iter().enumerate() {
        let on = i == d.selected;
        if on {
            selected_line = lines.len();
        }
        let binary = match &row.binary {
            Some(_) => ok("installed    ".into()),
            None => no("not on PATH  ".into()),
        };
        let hooks = match &row.hooks {
            Ok(true) => ok("hooks installed    ".into()),
            Ok(false) => warn("hooks not installed".into()),
            Err(_) => bad("hooks: error       ".into()),
        };
        let login = match &row.auth {
            Auth::LoggedIn(_) => ok("  login found".into()),
            Auth::NotFound => no("  no login found".into()),
            Auth::Unknown => no("  login: n/a".into()),
        };
        let mut name =
            Span::raw(format!(" {} {:<15}", if on { ">" } else { " " }, row.agent.label()));
        if on {
            name = name.style(reversed());
        }
        lines.push(Line::from(vec![name, Span::raw(" "), binary, Span::raw(" "), hooks, login]));
        if on {
            let dim = Style::new().fg(th.dim);
            if let Some(p) = &row.binary {
                lines
                    .push(Line::from(Span::styled(format!("       binary: {}", p.display()), dim)));
            }
            if let Some(p) = &row.config {
                lines
                    .push(Line::from(Span::styled(format!("       config: {}", p.display()), dim)));
            }
            if let Err(e) = &row.hooks {
                lines.push(Line::from(bad(format!("       {e}"))));
            }
            if let Auth::LoggedIn(p) = &row.auth {
                lines.push(Line::from(Span::styled(
                    format!("       login file: {}", p.display()),
                    dim,
                )));
            }
        }
    }

    lines.push(Line::raw(""));
    lines.push(heading("LLM providers: API keys in the session's environment"));
    for p in &d.snap.providers {
        let status = match p.set_var {
            Some(var) => ok(format!("set ({var})")),
            None => no(format!("not set ({})", p.vars.join(" / "))),
        };
        lines.push(Line::from(vec![Span::raw(format!("     {:<18}", p.name)), status]));
    }

    lines.push(Line::raw(""));
    lines.push(heading("yurlungr"));
    for (label, value) in &d.snap.info {
        let value =
            if label == "config warning" { warn(value.clone()) } else { Span::raw(value.clone()) };
        lines.push(Line::from(vec![
            Span::styled(format!("     {label:<20}"), Style::new().fg(th.dim)),
            value,
        ]));
    }

    let height = body.height as usize;
    if d.follow {
        keep_visible(&mut d.scroll, selected_line, height);
        keep_visible(&mut d.scroll, (selected_line + 4).min(lines.len().saturating_sub(1)), height);
    }
    d.scroll = d.scroll.min(lines.len().saturating_sub(height));
    f.render_widget(Paragraph::new(lines).scroll((d.scroll as u16, 0)), body);

    let status = match &d.message {
        Some(m) => Span::styled(format!(" {m}"), Style::new().fg(th.marker)),
        None => Span::styled(" Only existence of login files and API-key variables is checked; secrets are never read.", Style::new().fg(th.dim)),
    };
    let hints = Span::styled(" j/k select · i install hooks · u uninstall hooks · r refresh · PgUp/PgDn scroll · Esc close", Style::new().fg(th.dim));
    f.render_widget(Paragraph::new(vec![Line::from(status), Line::from(hints)]), footer);
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
        Overlay::Settings(d) => draw_settings(f, d, th),
        Overlay::Themes(d) => draw_themes(f, d, th),
    }
}
