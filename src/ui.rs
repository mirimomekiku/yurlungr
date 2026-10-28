use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Clear, List, ListItem, ListState, Paragraph},
    Frame,
};
use tui_term::widget::{Cursor, PseudoTerminal};

use crate::{
    app::App,
    terminal::Status,
    views::{FilesView, GraphView, Pane},
};

pub fn draw(f: &mut Frame, app: &mut App) {
    let [main, status] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(f.area());
    app.area = f.area();
    app.relayout(main);

    for (id, rect) in app.rects.clone() {
        let focused = id == app.focus;
        let Some(pane) = app.panes.get_mut(&id) else { continue };
        let mut title = Vec::new();
        if let Some(status) = pane.status() {
            let (word, color) = match status {
                Status::Working => (" ● working", Color::Green),
                Status::Blocked => (" ● blocked", Color::Yellow),
                Status::Idle => (" ○ idle", Color::DarkGray),
            };
            title.push(Span::styled(word, Style::new().fg(color).bold()));
            if let Some(agent) = pane.agent() {
                title.push(Span::styled(format!(" {agent}"), Style::new().fg(color)));
            }
        }
        title.push(Span::raw(pane.title()));
        let block = Block::bordered()
            .title(Line::from(title))
            .border_style(Style::new().fg(if focused { Color::Cyan } else { Color::DarkGray }));
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
            Pane::Graph(v) => render_graph(f, v, block, rect),
            Pane::Files(v) => render_files(f, v, block, rect),
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

    f.render_widget(Paragraph::new(status_line(app)).dim(), status);

    if let Some(menu) = &app.menu {
        let r = menu.rect(f.area());
        let items: Vec<ListItem> = menu.items.iter().map(|(l, _)| ListItem::new(*l)).collect();
        f.render_widget(Clear, r);
        f.render_stateful_widget(
            List::new(items)
                .block(Block::bordered().border_style(Style::new().fg(Color::Cyan)))
                .highlight_style(Style::new().add_modifier(Modifier::REVERSED)),
            r,
            &mut ListState::default().with_selected(Some(menu.selected)),
        );
    }
}

fn status_line(app: &App) -> String {
    if let Some(n) = &app.notice {
        return format!(" {n}");
    }
    if app.prefix {
        return " PREFIX  %/| split right · \"/- split down · x close · z zoom · o next · hjkl focus \
                · HJKL resize · g graph · f files · d detach · q quit session"
            .to_string();
    }
    let (working, blocked, idle) = app.status_counts();
    format!(
        " Ctrl+B prefix · right-click menu · drag to select · {working} working · {blocked} blocked · {idle} idle{}",
        if app.zoomed.is_some() { " · zoomed" } else { "" }
    )
}

fn render_graph(f: &mut Frame, v: &mut GraphView, block: Block, rect: Rect) {
    let highlight = Style::new().add_modifier(Modifier::REVERSED);
    match &v.rows {
        Ok(rows) => {
            let items: Vec<ListItem> = rows
                .iter()
                .map(|r| {
                    let mut spans = vec![
                        Span::styled(r.graph.clone(), Style::new().fg(Color::Green)),
                        Span::styled(format!("{} ", r.id), Style::new().fg(Color::Yellow)),
                    ];
                    if !r.refs.is_empty() {
                        spans.push(Span::styled(
                            format!("({}) ", r.refs.join(", ")),
                            Style::new().fg(Color::Magenta).bold(),
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

fn render_files(f: &mut Frame, v: &mut FilesView, block: Block, rect: Rect) {
    let items: Vec<ListItem> =
        v.fb.entries
            .iter()
            .map(|e| {
                if e.is_dir {
                    ListItem::new(format!("{}/", e.name)).style(Style::new().fg(Color::Blue).bold())
                } else {
                    ListItem::new(e.name.clone())
                }
            })
            .collect();
    f.render_stateful_widget(
        List::new(items)
            .block(block)
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED)),
        rect,
        &mut v.state,
    );
}
