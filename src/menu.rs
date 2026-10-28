use ratatui::layout::Rect;

use crate::layout::PaneId;

#[derive(Clone, Copy)]
pub enum MenuAction {
    SplitRight,
    SplitDown,
    NewGraph,
    NewFiles,
    Zoom,
    Close,
}

pub struct Menu {
    pub x: u16,
    pub y: u16,
    pub target: PaneId,
    pub items: Vec<(&'static str, MenuAction)>,
    pub selected: usize,
}

impl Menu {
    pub fn new(x: u16, y: u16, target: PaneId) -> Self {
        Self {
            x,
            y,
            target,
            selected: 0,
            items: vec![
                ("Split right", MenuAction::SplitRight),
                ("Split down", MenuAction::SplitDown),
                ("Git graph", MenuAction::NewGraph),
                ("File browser", MenuAction::NewFiles),
                ("Zoom / unzoom", MenuAction::Zoom),
                ("Close pane", MenuAction::Close),
            ],
        }
    }

    /// The popup rectangle, kept inside `screen`.
    pub fn rect(&self, screen: Rect) -> Rect {
        let label = self.items.iter().map(|(l, _)| l.len()).max().unwrap_or(0) as u16;
        let width = (label + 4).min(screen.width);
        let height = (self.items.len() as u16 + 2).min(screen.height);
        Rect {
            x: self.x.min(screen.right().saturating_sub(width)),
            y: self.y.min(screen.bottom().saturating_sub(height)),
            width,
            height,
        }
    }

    pub fn item_at(&self, x: u16, y: u16, screen: Rect) -> Option<usize> {
        let r = self.rect(screen);
        let inside = x > r.x && x + 1 < r.right() && y > r.y && y + 1 < r.bottom();
        let idx = (y.checked_sub(r.y + 1)?) as usize;
        (inside && idx < self.items.len()).then_some(idx)
    }
}
