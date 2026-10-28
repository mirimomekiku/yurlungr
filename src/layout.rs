use ratatui::layout::Rect;
use serde::{Deserialize, Serialize};

pub type PaneId = usize;

const MIN_SIZE: u16 = 4;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Dir {
    /// Children sit side by side.
    Horizontal,
    /// Children are stacked.
    Vertical,
}

#[derive(Clone, Serialize, Deserialize)]
pub enum Node {
    Leaf(PaneId),
    Split { dir: Dir, ratio: f32, a: Box<Node>, b: Box<Node> },
}

/// A draggable boundary between two children of a split.
pub struct Divider {
    pub path: Vec<bool>,
    pub dir: Dir,
    /// The area of the whole split, used to turn a mouse position into a ratio.
    pub area: Rect,
    pub rect: Rect,
}

fn split_area(dir: Dir, ratio: f32, area: Rect) -> (Rect, Rect) {
    match dir {
        Dir::Horizontal => {
            let lo = MIN_SIZE.min(area.width);
            let w = ((area.width as f32 * ratio).round() as u16)
                .max(lo)
                .min(area.width.saturating_sub(lo));
            (Rect { width: w, ..area }, Rect { x: area.x + w, width: area.width - w, ..area })
        }
        Dir::Vertical => {
            let lo = MIN_SIZE.min(area.height);
            let h = ((area.height as f32 * ratio).round() as u16)
                .max(lo)
                .min(area.height.saturating_sub(lo));
            (Rect { height: h, ..area }, Rect { y: area.y + h, height: area.height - h, ..area })
        }
    }
}

impl Node {
    pub fn layout(
        &self,
        area: Rect,
        path: &mut Vec<bool>,
        panes: &mut Vec<(PaneId, Rect)>,
        dividers: &mut Vec<Divider>,
    ) {
        match self {
            Node::Leaf(id) => panes.push((*id, area)),
            Node::Split { dir, ratio, a, b } => {
                let (ra, rb) = split_area(*dir, *ratio, area);
                let hit = match dir {
                    Dir::Horizontal => {
                        Rect { x: rb.x.saturating_sub(1), y: area.y, width: 2, height: area.height }
                    }
                    Dir::Vertical => {
                        Rect { x: area.x, y: rb.y.saturating_sub(1), width: area.width, height: 2 }
                    }
                };
                dividers.push(Divider { path: path.clone(), dir: *dir, area, rect: hit });
                path.push(false);
                a.layout(ra, path, panes, dividers);
                path.pop();
                path.push(true);
                b.layout(rb, path, panes, dividers);
                path.pop();
            }
        }
    }

    pub fn ids(&self, out: &mut Vec<PaneId>) {
        match self {
            Node::Leaf(id) => out.push(*id),
            Node::Split { a, b, .. } => {
                a.ids(out);
                b.ids(out);
            }
        }
    }

    pub fn split(&mut self, target: PaneId, new: PaneId, dir: Dir) -> bool {
        if matches!(self, Node::Leaf(id) if *id == target) {
            *self = Node::Split {
                dir,
                ratio: 0.5,
                a: Box::new(Node::Leaf(target)),
                b: Box::new(Node::Leaf(new)),
            };
            return true;
        }
        match self {
            Node::Leaf(_) => false,
            Node::Split { a, b, .. } => a.split(target, new, dir) || b.split(target, new, dir),
        }
    }

    /// Remove a leaf, promoting its sibling into the parent's place.
    pub fn remove(&mut self, target: PaneId) {
        if let Node::Split { a, b, .. } = self {
            if matches!(**a, Node::Leaf(id) if id == target) {
                let keep = std::mem::replace(&mut **b, Node::Leaf(0));
                *self = keep;
                return;
            }
            if matches!(**b, Node::Leaf(id) if id == target) {
                let keep = std::mem::replace(&mut **a, Node::Leaf(0));
                *self = keep;
                return;
            }
            a.remove(target);
            b.remove(target);
        }
    }

    pub fn path_to(&self, target: PaneId) -> Option<Vec<bool>> {
        match self {
            Node::Leaf(id) => (*id == target).then(Vec::new),
            Node::Split { a, b, .. } => {
                if let Some(mut p) = a.path_to(target) {
                    p.insert(0, false);
                    Some(p)
                } else if let Some(mut p) = b.path_to(target) {
                    p.insert(0, true);
                    Some(p)
                } else {
                    None
                }
            }
        }
    }

    fn node_at_mut(&mut self, path: &[bool]) -> &mut Node {
        let mut node = self;
        for &right in path {
            match node {
                Node::Split { a, b, .. } => node = if right { &mut **b } else { &mut **a },
                Node::Leaf(_) => break,
            }
        }
        node
    }

    pub fn set_ratio(&mut self, path: &[bool], new_ratio: f32) {
        if let Node::Split { ratio, .. } = self.node_at_mut(path) {
            *ratio = new_ratio.clamp(0.1, 0.9);
        }
    }

    /// Move the nearest divider of the given orientation around `target` by `delta`.
    pub fn nudge(&mut self, target: PaneId, axis: Dir, delta: f32) {
        let Some(path) = self.path_to(target) else { return };
        for depth in (0..path.len()).rev() {
            if let Node::Split { dir, ratio, .. } = self.node_at_mut(&path[..depth]) {
                if *dir == axis {
                    *ratio = (*ratio + delta).clamp(0.1, 0.9);
                    return;
                }
            }
        }
    }
}
