use ratatui::layout::{Position, Rect};

use crate::app::{ListPane, PaneId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    Confirm,
    Cancel,
}

/// A clickable action chip on a list pane's top border. Clicking one behaves
/// exactly like pressing its hotkey with that pane focused. `Create` ≡ `c`,
/// `Tasks` ≡ `t`, `Actions` ≡ `a`, `Collapse` ≡ `z` (labeled collapse/expand
/// by expanded/collapsed state).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneButton {
    Create,
    Tasks,
    Actions,
    Collapse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HitTarget {
    Tab(usize),
    Row(ListPane, usize),
    PaneBody(PaneId),
    SubTab(usize),
    MenuItem(usize),
    FormField(usize),
    DropdownItem(usize),
    Button(ButtonKind),
    ScrollbarThumb(PaneId),
    ScrollbarTrack(PaneId),
    /// Draggable boundary between two stacked left panes: `0` = queue/tasks,
    /// `1` = tasks/worktrees. Covers the two shared border rows, full pane width.
    PaneDividerH(usize),
    /// Draggable boundary column between the left pane stack and DETAIL.
    PaneDividerV,
    /// An action chip on a list pane's top border. Registered LAST so a chip
    /// click wins its sub-rect over the divider band sharing the border row.
    /// The rest of the title row deliberately has no click target — a whole-row
    /// collapse toggle used to live there and swallowed divider drags (collapse
    /// ≡ the [z] collapse chip or the `z` key).
    PaneButton(PaneId, PaneButton),
    /// The picker's right (preview) panel interior. Clicks are inert (like
    /// `Modal`); the mouse wheel over it scrolls the preview instead of moving
    /// the list selection.
    MenuPreview,
    Modal,
}

/// Ordered registry of `(Rect, HitTarget)`. Elements are registered painter's-
/// order (background first, modals last); `hit` scans in reverse so the topmost
/// (last-registered) element under a point wins — clicks never leak through a
/// modal into the body beneath it.
#[derive(Debug, Default, Clone)]
pub struct HitMap {
    entries: Vec<(Rect, HitTarget)>,
}

impl HitMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, rect: Rect, target: HitTarget) {
        self.entries.push((rect, target));
    }

    /// Topmost target containing `(col, row)`, or `None`. Uses `Rect::contains`
    /// (ratatui 0.29): a point is inside iff `x ∈ [x, x+width)` and
    /// `y ∈ [y, y+height)` — the right/bottom edges are exclusive, zero-sized
    /// rects contain nothing.
    pub fn hit(&self, col: u16, row: u16) -> Option<&HitTarget> {
        let p = Position { x: col, y: row };
        self.entries
            .iter()
            .rev()
            .find(|(rect, _)| rect.contains(p))
            .map(|(_, target)| target)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &(Rect, HitTarget)> {
        self.entries.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{ListPane, PaneId};
    use ratatui::layout::Rect;

    fn r(x: u16, y: u16, w: u16, h: u16) -> Rect { Rect { x, y, width: w, height: h } }

    #[test]
    fn empty_map_hits_nothing() {
        let m = HitMap::new();
        assert_eq!(m.hit(0, 0), None);
        assert!(m.is_empty());
    }

    #[test]
    fn single_rect_inside_and_outside() {
        let mut m = HitMap::new();
        m.push(r(2, 3, 5, 4), HitTarget::Tab(1));
        assert_eq!(m.hit(2, 3), Some(&HitTarget::Tab(1))); // top-left corner inside
        assert_eq!(m.hit(6, 6), Some(&HitTarget::Tab(1))); // bottom-right inside (x<7,y<7)
        assert_eq!(m.hit(7, 3), None);                     // x == right edge is outside
        assert_eq!(m.hit(2, 7), None);                     // y == bottom edge is outside
        assert_eq!(m.hit(1, 3), None);                     // left of rect
    }

    #[test]
    fn overlap_resolves_to_last_registered() {
        let mut m = HitMap::new();
        m.push(r(0, 0, 10, 10), HitTarget::PaneBody(PaneId::Queue)); // background
        m.push(r(2, 2, 4, 4), HitTarget::Row(ListPane::Queue, 3));   // foreground row
        m.push(r(0, 0, 10, 10), HitTarget::Modal);                   // modal registered LAST
        // Modal covers everything and wins because hit() scans in reverse.
        assert_eq!(m.hit(3, 3), Some(&HitTarget::Modal));
        assert_eq!(m.hit(8, 8), Some(&HitTarget::Modal));
    }

    #[test]
    fn foreground_wins_over_background_without_modal() {
        let mut m = HitMap::new();
        m.push(r(0, 0, 10, 10), HitTarget::PaneBody(PaneId::Queue));
        m.push(r(2, 2, 4, 4), HitTarget::Row(ListPane::Queue, 3));
        assert_eq!(m.hit(3, 3), Some(&HitTarget::Row(ListPane::Queue, 3)));
        assert_eq!(m.hit(9, 9), Some(&HitTarget::PaneBody(PaneId::Queue)));
    }

    #[test]
    fn zero_sized_rect_never_hits() {
        let mut m = HitMap::new();
        m.push(r(5, 5, 0, 3), HitTarget::Button(ButtonKind::Confirm));
        m.push(r(5, 5, 3, 0), HitTarget::Button(ButtonKind::Cancel));
        assert_eq!(m.hit(5, 5), None);
    }
}
