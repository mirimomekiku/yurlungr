use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
    Frame,
};
use tui_term::widget::{Cursor, PseudoTerminal};
use unicode_width::UnicodeWidthStr;

use crate::{
    app::{App, DockHit},
    editor::{self, EditorView, Mode},
    git_graph::{CommitDetail, FileChange},
    keymap::Action,
    overlay,
    terminal::Status,
    theme::Theme,
    views::{Clip, FilesView, GraphView, Pane},
    workspace::AgentState,
};

const NEW_BUTTON: &str = " + space ";
const MENU_BUTTON: &str = " ≡ menu ";
/// Width of the left sidebar listing panes, and the narrowest screen that gets one.
const SIDEBAR_WIDTH: u16 = 26;
const SIDEBAR_MIN_SCREEN: u16 = 60;
/// Lines of a commit message shown in the graph's detail panel.
const MESSAGE_PREVIEW_LINES: usize = 4;

fn sidebar_width(screen_width: u16) -> u16 {
    if screen_width >= SIDEBAR_MIN_SCREEN {
        SIDEBAR_WIDTH
    } else {
        0
    }
}

pub fn draw(f: &mut Frame, app: &mut App) {
    // Sidebar on the left; the hint line stays at the bottom across the full width.
    let [body, status] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(f.area());
    let [sidebar, main] =
        Layout::horizontal([Constraint::Length(sidebar_width(body.width)), Constraint::Min(1)])
            .areas(body);
    app.area = f.area();
    app.relayout(main);
    let th = app.theme;

    for (id, rect) in app.rects.clone() {
        let focused = id == app.focus;
        let Some(pane) = app.panes.get_mut(&id) else { continue };
        let mut title = Vec::new();
        if let Some(status) = pane.status() {
            let (word, color) = match status {
                Status::Working => (" ● working", th.working),
                Status::Blocked => (" ● blocked", th.blocked),
                Status::Idle => (" ○ idle", th.idle),
            };
            title.push(Span::styled(word, Style::new().fg(color).bold()));
            if let Some(agent) = pane.agent() {
                title.push(Span::styled(format!(" {agent}"), Style::new().fg(color)));
            }
        }
        title.push(Span::raw(pane.title()));
        let block = Block::bordered()
            .title(Line::from(title))
            .border_style(Style::new().fg(if focused { th.accent } else { th.border }));
        match pane {
            Pane::Terminal(t) => {
                let inner = block.inner(rect);
                t.resize(inner.height, inner.width);
                t.with_screen(|screen| {
                    let term = PseudoTerminal::new(screen)
                        .block(block)
                        .cursor(Cursor::default().visibility(focused));
                    f.render_widget(term, rect);
                });
            }
            Pane::Graph(v) => render_graph(f, v, block, rect, &th),
            Pane::Files(v) => render_files(f, v, block, rect, app.file_clip.as_ref(), &th),
            Pane::Editor(v) => render_editor(f, v, block, rect, focused, &th),
        }
    }

    if let Some(sel) = &app.sel {
        if let Some(&(_, rect)) = app.rects.iter().find(|(id, _)| *id == sel.pane) {
            let a = Block::bordered().inner(rect);
            let ((r0, c0), (r1, c1)) = sel.ordered();
            for r in r0..=r1 {
                let from = if r == r0 { c0 } else { 0 };
                let to = if r == r1 { c1 } else { a.width.saturating_sub(1) };
                for c in from..=to {
                    if let Some(cell) = f.buffer_mut().cell_mut((a.x + c, a.y + r)) {
                        cell.set_style(Style::new().add_modifier(Modifier::REVERSED));
                    }
                }
            }
        }
    }

    draw_sidebar(f, app, sidebar);
    f.render_widget(Paragraph::new(status_line(app)).dim(), status);

    if let Some(menu) = &app.menu {
        let r = menu.rect(f.area());
        let items: Vec<ListItem> = menu.items.iter().map(|(l, _)| ListItem::new(*l)).collect();
        f.render_widget(Clear, r);
        f.render_stateful_widget(
            List::new(items)
                .block(Block::bordered().border_style(Style::new().fg(th.accent)))
                .highlight_style(Style::new().add_modifier(Modifier::REVERSED)),
            r,
            &mut ListState::default().with_selected(Some(menu.selected)),
        );
    }

    if let Some(mut dialog) = app.overlay.take() {
        overlay::draw(f, &mut dialog, &th, &app.config.keymap);
        app.overlay = Some(dialog);
    }
}

fn clip_name(name: &str, max: usize) -> String {
    if name.chars().count() <= max {
        return name.to_string();
    }
    let mut out: String = name.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Marker and colour for an agent state; `None` means the workspace has no agents.
fn state_mark(state: Option<AgentState>, th: &Theme) -> (&'static str, ratatui::style::Color) {
    match state {
        Some(AgentState::Working) => ("●", th.working),
        Some(AgentState::Blocked) => ("●", th.blocked),
        Some(AgentState::Done) => ("✓", th.accent),
        Some(AgentState::Idle) => ("○", th.idle),
        None => ("·", th.dim),
    }
}

/// First visible row of a list of `len` rows in a window of `visible` rows that
/// keeps `selected` in view.
fn window_start(selected: usize, visible: usize) -> usize {
    (selected + 1).saturating_sub(visible)
}

enum AgentRow {
    /// The workspace the agents below it belong to.
    Header(String),
    Agent(crate::app::AgentEntry),
}

/// The always-visible sidebar, like herdr's: a "spaces" list of workspaces (each
/// showing the most urgent state of its agents) over an "agents" list of every
/// agent in every workspace. The new/menu buttons sit at the bottom. Publishes
/// click targets in `app.dock`.
fn draw_sidebar(f: &mut Frame, app: &mut App, area: Rect) {
    app.dock.clear();
    if area.width == 0 || area.height < 9 {
        return;
    }
    let th = app.theme;
    let body_h = area.height - 1;
    // Spaces and agents each get half of the height (the button row is the odd one out).
    let spaces_h = body_h / 2;
    let [spaces_area, agents_area, buttons_area] =
        Layout::vertical([Constraint::Length(spaces_h), Constraint::Min(3), Constraint::Length(1)])
            .areas(area);

    // ---- spaces
    let block = Block::bordered().title(" spaces ").border_style(Style::new().fg(th.border));
    let inner = block.inner(spaces_area);
    f.render_widget(block, spaces_area);
    let visible = inner.height as usize;
    let start = window_start(app.active, visible);
    for (row, i) in (start..app.workspaces.len()).take(visible).enumerate() {
        let (mark, color) = state_mark(app.workspace_state(i), &th);
        let base = if i == app.active {
            Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD)
        } else {
            Style::new()
        };
        let name = clip_name(&app.workspaces[i].name, (inner.width as usize).saturating_sub(4));
        let line = Line::from(vec![
            Span::styled(" ", base),
            Span::styled(format!("{mark} "), base.fg(color)),
            Span::styled(name, base),
        ]);
        let rect = Rect::new(inner.x, inner.y + row as u16, inner.width, 1);
        f.render_widget(Paragraph::new(line).style(base), rect);
        app.dock.push((rect, DockHit::Workspace(i)));
    }

    // ---- agents
    let agents = app.agents();
    let block = Block::bordered()
        .title(format!(
            " agents{} ",
            if agents.is_empty() { String::new() } else { format!(" {}", agents.len()) }
        ))
        .border_style(Style::new().fg(th.border));
    let inner = block.inner(agents_area);
    f.render_widget(block, agents_area);
    if agents.is_empty() {
        let dim = Style::new().fg(th.dim);
        f.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(" none running", dim)),
                Line::from(Span::styled(" try claude, codex…", dim)),
            ]),
            inner,
        );
    } else {
        // Group under workspace headers once agents live in more than one.
        let mut spaces_with_agents: Vec<usize> = agents.iter().map(|a| a.workspace).collect();
        spaces_with_agents.dedup();
        let grouped = spaces_with_agents.len() > 1;
        let mut rows: Vec<AgentRow> = Vec::new();
        let mut last = None;
        for entry in agents {
            if grouped && last != Some(entry.workspace) {
                rows.push(AgentRow::Header(app.workspaces[entry.workspace].name.clone()));
            }
            last = Some(entry.workspace);
            rows.push(AgentRow::Agent(entry));
        }
        let current = rows
            .iter()
            .position(|r| matches!(r, AgentRow::Agent(a) if a.pane == app.focus && a.workspace == app.active))
            .unwrap_or(0);
        let visible = inner.height as usize;
        let start = window_start(current, visible);
        for (row, entry) in rows.iter().enumerate().skip(start).take(visible) {
            let rect = Rect::new(inner.x, inner.y + (row - start) as u16, inner.width, 1);
            match entry {
                AgentRow::Header(name) => {
                    let text = clip_name(name, (inner.width as usize).saturating_sub(2));
                    f.render_widget(
                        Paragraph::new(Line::from(Span::styled(
                            format!(" {text}"),
                            Style::new().fg(th.dim).add_modifier(Modifier::BOLD),
                        ))),
                        rect,
                    );
                }
                AgentRow::Agent(a) => {
                    let focused = a.pane == app.focus && a.workspace == app.active;
                    let base = if focused {
                        Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD)
                    } else {
                        Style::new()
                    };
                    let (mark, color) = state_mark(Some(a.state), &th);
                    let word = a.state.label();
                    let width = inner.width as usize;
                    let room = width.saturating_sub(4 + word.width() + 1);
                    let name = clip_name(&a.name, room.max(3));
                    let pad = width.saturating_sub(3 + name.width() + word.width() + 1);
                    let line = Line::from(vec![
                        Span::styled(" ", base),
                        Span::styled(format!("{mark} "), base.fg(color)),
                        Span::styled(name, base),
                        Span::styled(" ".repeat(pad), base),
                        Span::styled(word, if focused { base } else { Style::new().fg(th.dim) }),
                    ]);
                    f.render_widget(Paragraph::new(line).style(base), rect);
                    app.dock.push((rect, DockHit::Agent(a.pane)));
                }
            }
        }
    }

    // ---- buttons
    let y = buttons_area.y;
    let new_rect = Rect::new(buttons_area.x + 1, y, NEW_BUTTON.width() as u16, 1);
    let menu_rect = Rect::new(new_rect.right() + 1, y, MENU_BUTTON.width() as u16, 1);
    if menu_rect.right() <= buttons_area.right() {
        f.render_widget(
            Paragraph::new(NEW_BUTTON)
                .style(Style::new().fg(th.button_fg).bg(th.button_new).bold()),
            new_rect,
        );
        f.render_widget(
            Paragraph::new(MENU_BUTTON)
                .style(Style::new().fg(th.button_fg).bg(th.button_menu).bold()),
            menu_rect,
        );
        app.dock.push((new_rect, DockHit::NewWorkspace));
        app.dock.push((menu_rect, DockHit::Menu));
    }
}

/// "key action" pairs for the status line, from the live keymap.
fn hints(app: &App, actions: &[Action], prefixed: Option<bool>) -> String {
    let km = &app.config.keymap;
    let mut seen = Vec::new();
    let mut out = Vec::new();
    for &a in actions {
        let Some(chord) = km.chords(a).iter().find(|c| prefixed.is_none_or(|p| c.prefix == p))
        else {
            continue;
        };
        let short = a.short();
        if seen.contains(&short) {
            continue;
        }
        out.push(format!("{} {short}", chord.key.format()));
        seen.push(short);
    }
    out.join(" · ")
}

fn status_line(app: &App) -> String {
    if let Some(n) = &app.notice {
        return format!(" {n}");
    }
    if app.overlay.is_some() {
        return String::new();
    }
    let km = &app.config.keymap;
    if app.prefix {
        let global: Vec<Action> = Action::all()
            .into_iter()
            .filter(|a| a.scope() == crate::keymap::Scope::Global)
            .collect();
        return format!(" PREFIX  {}", hints(app, &global, Some(true)));
    }
    match app.panes.get(&app.focus) {
        Some(Pane::Files(v)) if v.prompt.is_some() => {
            return " Enter confirm · Esc cancel".to_string();
        }
        Some(Pane::Files(_)) => {
            let acts: Vec<Action> = Action::all()
                .into_iter()
                .filter(|a| a.scope() == crate::keymap::Scope::Files)
                .collect();
            return format!(" Enter open · {}", hints(app, &acts, Some(false)));
        }
        Some(Pane::Editor(v)) => {
            let acts = match v.mode {
                Mode::View => [Action::EditorStartEdit, Action::EditorSave, Action::EditorReload],
                Mode::Edit => [Action::EditorStopEdit, Action::EditorSave, Action::EditorSave],
            };
            return format!(
                " {}: {}",
                if v.mode == Mode::Edit { "edit" } else { "view" },
                hints(app, &acts, Some(false))
            );
        }
        _ => {}
    }
    let (working, blocked, idle) = app.status_counts();
    format!(
        " {} prefix · split {} {} {} · pane {} · space {} · menu: {} · {working} working · {blocked} blocked · {idle} idle{}",
        km.prefix.format(),
        km.hint(Action::SplitLeft),
        km.hint(Action::SplitRight),
        km.hint(Action::SplitDown),
        km.hint(Action::GotoPane(1)).replace('1', "N"),
        km.hint(Action::GotoWorkspace(1)).replace('1', "N"),
        km.hint(Action::OpenMenu),
        if app.zoomed.is_some() { " · zoomed" } else { "" }
    )
}

fn render_graph(f: &mut Frame, v: &mut GraphView, block: Block, rect: Rect, th: &Theme) {
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    // The detail panel needs room; on a short pane only the list is shown.
    let detail_height =
        if v.show_detail && inner.height >= 14 { (inner.height / 2).clamp(8, 24) } else { 0 };
    let [list_area, detail_area] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(detail_height)]).areas(inner);
    v.list_area = list_area;
    v.detail_area = detail_area;

    let highlight = Style::new().add_modifier(Modifier::REVERSED);
    match &v.rows {
        Ok(rows) => {
            let items: Vec<ListItem> = rows
                .iter()
                .map(|r| {
                    let mut spans = vec![
                        Span::styled(r.graph.clone(), Style::new().fg(th.graph_lane)),
                        Span::styled(format!("{} ", r.id), Style::new().fg(th.graph_hash)),
                    ];
                    if !r.refs.is_empty() {
                        spans.push(Span::styled(
                            format!("({}) ", r.refs.join(", ")),
                            Style::new().fg(th.graph_ref).bold(),
                        ));
                    }
                    spans.push(Span::raw(r.summary.clone()));
                    spans.push(Span::styled(
                        format!("  {} · {}", r.author, r.date),
                        Style::new().fg(th.dim),
                    ));
                    ListItem::new(Line::from(spans))
                })
                .collect();
            f.render_stateful_widget(
                List::new(items).highlight_style(highlight),
                list_area,
                &mut v.state,
            );
        }
        Err(e) => {
            f.render_widget(Paragraph::new(e.as_str()), inner);
            return;
        }
    }

    if detail_height > 0 {
        v.ensure_detail();
        let refs = v.selected_row().map(|r| r.refs.clone()).unwrap_or_default();
        let panel = Block::default()
            .borders(Borders::TOP)
            .title(" commit ")
            .border_style(Style::new().fg(th.border));
        let body = panel.inner(detail_area);
        f.render_widget(panel, detail_area);
        let lines = match v.detail() {
            Some(Ok(d)) => detail_lines(d, &refs, body.width as usize, th),
            Some(Err(e)) => {
                vec![Line::from(Span::styled(format!(" {e}"), Style::new().fg(th.danger)))]
            }
            None => Vec::new(),
        };
        v.detail_scroll = v.detail_scroll.min(lines.len().saturating_sub(body.height as usize));
        f.render_widget(Paragraph::new(lines).scroll((v.detail_scroll as u16, 0)), body);
    }
}

/// Keep the end of a long path, which is the informative part.
fn clip_left(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let tail: String = text.chars().skip(count - max.saturating_sub(1)).collect();
    format!("…{tail}")
}

fn file_line(file: &FileChange, width: usize, th: &Theme) -> Line<'static> {
    let color = match file.status {
        'A' => th.working,
        'D' => th.danger,
        'M' => th.marker,
        'R' | 'C' => th.accent,
        _ => th.dim,
    };
    let stat = if file.binary {
        vec![Span::styled("binary", Style::new().fg(th.dim))]
    } else {
        vec![
            Span::styled(format!("+{}", file.added), Style::new().fg(th.working)),
            Span::raw(" "),
            Span::styled(format!("-{}", file.deleted), Style::new().fg(th.danger)),
        ]
    };
    let stat_width: usize = stat.iter().map(|s| s.content.width()).sum();
    let room = width.saturating_sub(4 + stat_width + 1);
    let path = clip_left(&file.path, room);
    let pad = width.saturating_sub(3 + path.width() + stat_width + 1);
    let mut spans = vec![
        Span::raw(" "),
        Span::styled(file.status.to_string(), Style::new().fg(color).bold()),
        Span::raw(" "),
        Span::raw(path),
        Span::raw(" ".repeat(pad)),
    ];
    spans.extend(stat);
    Line::from(spans)
}

/// The text of the commit detail panel: who, when, what, and the files changed.
fn detail_lines(d: &CommitDetail, refs: &[String], width: usize, th: &Theme) -> Vec<Line<'static>> {
    let label = |text: &str| Span::styled(format!(" {text:<10}"), Style::new().fg(th.dim));
    let mut lines = Vec::new();

    let mut head =
        vec![label("commit"), Span::styled(d.id.clone(), Style::new().fg(th.graph_hash).bold())];
    if !refs.is_empty() {
        head.push(Span::styled(
            format!("  ({})", refs.join(", ")),
            Style::new().fg(th.graph_ref).bold(),
        ));
    }
    lines.push(Line::from(head));
    lines.push(Line::from(vec![
        label("author"),
        Span::raw(format!("{}  {}", d.author, d.author_time)),
    ]));
    lines.push(Line::from(vec![
        label("committer"),
        Span::raw(format!("{}  {}", d.committer, d.commit_time)),
    ]));
    let parents =
        if d.parents.is_empty() { "none (root commit)".to_string() } else { d.parents.join(" ") };
    lines.push(Line::from(vec![label("parents"), Span::raw(parents)]));
    lines.push(Line::raw(""));

    // A long body would push the file list out of view, so only a preview is shown.
    let message: Vec<&str> = d.message.lines().collect();
    for (i, text) in message.iter().take(MESSAGE_PREVIEW_LINES).enumerate() {
        let style = if i == 0 { Style::new().bold() } else { Style::new() };
        lines.push(Line::from(Span::styled(format!("  {text}"), style)));
    }
    if message.len() > MESSAGE_PREVIEW_LINES {
        lines.push(Line::from(Span::styled(
            format!("  … {} more lines", message.len() - MESSAGE_PREVIEW_LINES),
            Style::new().fg(th.dim),
        )));
    }
    lines.push(Line::raw(""));

    let noun = if d.files_total == 1 { "file" } else { "files" };
    let mut summary =
        vec![Span::styled(format!(" {} {noun} changed", d.files_total), Style::new().bold())];
    if let Some((added, deleted)) = d.lines {
        summary.push(Span::raw(", "));
        summary.push(Span::styled(format!("+{added}"), Style::new().fg(th.working)));
        summary.push(Span::raw(" "));
        summary.push(Span::styled(format!("-{deleted}"), Style::new().fg(th.danger)));
    } else {
        summary.push(Span::styled(" (too large to count lines)", Style::new().fg(th.dim)));
    }
    if d.parents.len() > 1 {
        summary.push(Span::styled("  (against first parent)", Style::new().fg(th.dim)));
    }
    lines.push(Line::from(summary));
    lines.extend(d.files.iter().map(|f| file_line(f, width, th)));
    if d.files_total > d.files.len() {
        lines.push(Line::from(Span::styled(
            format!(" … and {} more files", d.files_total - d.files.len()),
            Style::new().fg(th.dim),
        )));
    }
    lines
}

fn render_files(
    f: &mut Frame,
    v: &mut FilesView,
    block: Block,
    rect: Rect,
    clip: Option<&Clip>,
    th: &Theme,
) {
    let items: Vec<ListItem> =
        v.fb.entries
            .iter()
            .map(|e| {
                let marked = clip.filter(|c| c.path == v.fb.dir.join(&e.name));
                let mut spans = vec![if e.is_dir {
                    Span::styled(format!("{}/", e.name), Style::new().fg(th.dir).bold())
                } else {
                    Span::raw(e.name.clone())
                }];
                if let Some(c) = marked {
                    spans.push(Span::styled(
                        if c.cut { "  (cut)" } else { "  (copied)" },
                        Style::new().fg(th.marker).dim(),
                    ));
                }
                ListItem::new(Line::from(spans))
            })
            .collect();

    // While a prompt is open it takes the last row of the pane.
    let list_rect = if v.prompt.is_some() && rect.height > 3 {
        Rect { height: rect.height - 1, ..rect }
    } else {
        rect
    };
    f.render_stateful_widget(
        List::new(items)
            .block(block)
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED)),
        list_rect,
        &mut v.state,
    );

    if let Some(p) = &v.prompt {
        let row = Rect { y: rect.bottom().saturating_sub(1), height: 1, ..rect };
        let line = if p.is_confirm() {
            Line::from(Span::styled(format!(" {}", p.label()), Style::new().fg(th.danger).bold()))
        } else {
            let label = format!(" {}: ", p.label());
            let room = (rect.width as usize).saturating_sub(label.width() + 2);
            let chars: Vec<char> = p.text.chars().collect();
            let shown: String = chars[chars.len().saturating_sub(room)..].iter().collect();
            Line::from(vec![
                Span::styled(label, Style::new().fg(th.marker).bold()),
                Span::raw(shown),
                Span::styled("▏", Style::new().fg(th.marker)),
            ])
        };
        f.render_widget(Clear, row);
        f.render_widget(Paragraph::new(line), row);
    }
}

fn render_editor(
    f: &mut Frame,
    v: &mut EditorView,
    block: Block,
    rect: Rect,
    focused: bool,
    th: &Theme,
) {
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let gutter = v.gutter();
    v.set_viewport(inner.height as usize, (inner.width as usize).saturating_sub(gutter));
    if inner.width as usize <= gutter || inner.height == 0 {
        return;
    }
    v.ensure_visible();

    let number_width = gutter - 1;
    let lines: Vec<Line> = (v.top..(v.top + v.height).min(v.lines.len()))
        .map(|i| {
            let number_style = if i == v.row {
                Style::new().fg(th.marker).bold()
            } else {
                Style::new().fg(th.dim)
            };
            Line::from(vec![
                Span::styled(format!("{:>number_width$} ", i + 1), number_style),
                Span::raw(editor::visible_slice(&v.lines[i], v.left, v.width)),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);

    if !focused {
        return;
    }
    let y = inner.y + (v.row - v.top) as u16;
    match v.mode {
        Mode::Edit => {
            let dx = editor::display_col(&v.lines[v.row], v.col) - v.left;
            let x = inner.x + (gutter + dx) as u16;
            if let Some(cell) = f.buffer_mut().cell_mut((x, y)) {
                cell.set_style(Style::new().add_modifier(Modifier::REVERSED));
            }
        }
        Mode::View => {
            for x in inner.x + gutter as u16..inner.right() {
                if let Some(cell) = f.buffer_mut().cell_mut((x, y)) {
                    cell.set_bg(th.line_bg);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidebar_only_appears_on_screens_wide_enough() {
        assert_eq!(sidebar_width(59), 0);
        assert_eq!(sidebar_width(60), SIDEBAR_WIDTH);
        assert_eq!(sidebar_width(200), SIDEBAR_WIDTH);
    }

    #[test]
    fn long_paths_keep_their_tail() {
        assert_eq!(clip_left("src/app.rs", 20), "src/app.rs");
        assert_eq!(clip_left("src/deeply/nested/app.rs", 10), "…ed/app.rs");
        assert_eq!(clip_left("abcdef", 4), "…def");
        assert_eq!(clip_left("abcdef", 1), "…");
    }
}
