use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Clear, List, ListItem, ListState, Paragraph},
    Frame,
};
use tui_term::widget::{Cursor, PseudoTerminal};
use unicode_width::UnicodeWidthStr;

use crate::{
    app::{App, DockHit},
    editor::{self, EditorView, Mode},
    keymap::Action,
    overlay,
    terminal::Status,
    theme::Theme,
    views::{Clip, FilesView, GraphView, Pane},
};

const NEW_BUTTON: &str = " + new ";
const MENU_BUTTON: &str = " ≡ menu ";
/// Longest pane name shown in the dock before it is cut with an ellipsis.
const DOCK_NAME_MAX: usize = 16;

pub fn draw(f: &mut Frame, app: &mut App) {
    let [main, dock, status] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1), Constraint::Length(1)])
            .areas(f.area());
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

    draw_dock(f, app, dock);
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

/// The always-visible bar of every pane ("space") plus the new/menu buttons.
/// Publishes click targets in `app.dock`.
fn draw_dock(f: &mut Frame, app: &mut App, area: Rect) {
    app.dock.clear();

    let buttons_w = (NEW_BUTTON.width() + 1 + MENU_BUTTON.width()) as u16;
    let menu_x = area.right().saturating_sub(MENU_BUTTON.width() as u16);
    let new_x = menu_x.saturating_sub(NEW_BUTTON.width() as u16 + 1);
    let new_rect = Rect::new(new_x, area.y, NEW_BUTTON.width() as u16, 1);
    let menu_rect = Rect::new(menu_x, area.y, MENU_BUTTON.width() as u16, 1);
    if area.width > buttons_w {
        f.render_widget(
            Paragraph::new(NEW_BUTTON)
                .style(Style::new().fg(app.theme.button_fg).bg(app.theme.button_new).bold()),
            new_rect,
        );
        f.render_widget(
            Paragraph::new(MENU_BUTTON)
                .style(Style::new().fg(app.theme.button_fg).bg(app.theme.button_menu).bold()),
            menu_rect,
        );
        app.dock.push((new_rect, DockHit::New));
        app.dock.push((menu_rect, DockHit::Menu));
    }

    struct Item {
        id: usize,
        number: String,
        glyph: Option<(&'static str, Color)>,
        name: String,
    }
    impl Item {
        fn width(&self) -> usize {
            // " {number} {glyph}{name} "
            self.number.width()
                + 3
                + self.glyph.map_or(0, |(g, _)| g.width() + 1)
                + self.name.width()
        }
    }

    let ids = app.pane_order();
    let items: Vec<Item> = ids
        .iter()
        .enumerate()
        .filter_map(|(i, id)| {
            let pane = app.panes.get(id)?;
            let glyph = pane.status().map(|s| match s {
                Status::Working => ("●", app.theme.working),
                Status::Blocked => ("●", app.theme.blocked),
                Status::Idle => ("○", app.theme.idle),
            });
            Some(Item {
                id: *id,
                number: (i + 1).to_string(),
                glyph,
                name: clip_name(&pane.label(), DOCK_NAME_MAX),
            })
        })
        .collect();

    // Slide the window of visible entries so the focused one is always shown.
    const MORE: usize = 5; // " +NN "
    let avail = area.width.saturating_sub(buttons_w + 1) as usize;
    let widths: Vec<usize> = items.iter().map(Item::width).collect();
    let focus_idx = items.iter().position(|it| it.id == app.focus).unwrap_or(0);
    let mut start = 0;
    while start < focus_idx {
        let lead = if start > 0 { 2 } else { 0 };
        let tail = if focus_idx + 1 < items.len() { MORE } else { 0 };
        if widths[start..=focus_idx].iter().sum::<usize>() + lead + tail <= avail {
            break;
        }
        start += 1;
    }

    let mut spans: Vec<Span> = Vec::new();
    let mut x = 0usize;
    if start > 0 {
        spans.push(Span::styled("‹ ", Style::new().fg(app.theme.dim)));
        x += 2;
    }
    let mut shown_to = start;
    for (i, item) in items.iter().enumerate().skip(start) {
        let tail = if i + 1 < items.len() { MORE } else { 0 };
        if x + widths[i] + tail > avail {
            break;
        }
        let base = if item.id == app.focus {
            Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD)
        } else {
            Style::new()
        };
        spans.push(Span::styled(format!(" {} ", item.number), base));
        if let Some((g, color)) = item.glyph {
            spans.push(Span::styled(format!("{g} "), base.fg(color)));
        }
        spans.push(Span::styled(format!("{} ", item.name), base));
        app.dock.push((
            Rect::new(area.x + x as u16, area.y, widths[i] as u16, 1),
            DockHit::Pane(item.id),
        ));
        x += widths[i];
        shown_to = i + 1;
    }
    if shown_to < items.len() {
        spans.push(Span::styled(
            format!(" +{} ", items.len() - shown_to),
            Style::new().fg(app.theme.dim),
        ));
    }
    let list_area = Rect { width: area.width.saturating_sub(buttons_w + 1), ..area };
    f.render_widget(Paragraph::new(Line::from(spans)), list_area);
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
        let all = Action::all();
        return format!(" PREFIX  {}", hints(app, &all[..24], Some(true)));
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
        " {} prefix · split {} {} {} · jump {} · menu: {} · {working} working · {blocked} blocked · {idle} idle{}",
        km.prefix.format(),
        km.hint(Action::SplitLeft),
        km.hint(Action::SplitRight),
        km.hint(Action::SplitDown),
        km.hint(Action::GotoPane(1)).replace('1', "N"),
        km.hint(Action::OpenMenu),
        if app.zoomed.is_some() { " · zoomed" } else { "" }
    )
}

fn render_graph(f: &mut Frame, v: &mut GraphView, block: Block, rect: Rect, th: &Theme) {
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
                    spans.push(Span::styled(format!("  {}", r.author), Style::new().dim()));
                    ListItem::new(Line::from(spans))
                })
                .collect();
            f.render_stateful_widget(
                List::new(items).block(block).highlight_style(highlight),
                rect,
                &mut v.state,
            );
        }
        Err(e) => f.render_widget(Paragraph::new(e.as_str()).block(block), rect),
    }
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
