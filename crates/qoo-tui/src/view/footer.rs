use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;

use crate::app::{App, ListPane, PaneId};
use crate::view::theme::Palette;
use crate::view::{Computed, is_bulk_selection, selected_positions};

/// Style a hint string so the key tokens stand out instead of the whole line
/// rendering dim: `[key]` chunks get the accent color (bold), `·` separators
/// stay dim, and the remaining label text uses the normal foreground.
fn hint_line(s: &str, p: &Palette) -> Line<'static> {
    let key_style = Style::default().fg(p.accent).add_modifier(Modifier::BOLD);
    let label_style = Style::default().fg(p.fg);
    let mut spans: Vec<Span> = Vec::new();
    let mut label = String::new();
    let mut key: Option<String> = None;
    for ch in s.chars() {
        match (ch, &mut key) {
            ('[', None) => {
                if !label.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut label), label_style));
                }
                key = Some(String::new());
            }
            (']', Some(k)) => {
                spans.push(Span::styled(format!("[{k}]"), key_style));
                key = None;
            }
            ('·', None) => {
                if !label.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut label), label_style));
                }
                spans.push(Span::styled("·".to_string(), p.dim_style()));
            }
            (_, Some(k)) => k.push(ch),
            (_, None) => label.push(ch),
        }
    }
    // Unterminated `[...` (never in practice) renders literally as label text.
    if let Some(k) = key {
        label.push('[');
        label.push_str(&k);
    }
    if !label.is_empty() {
        spans.push(Span::styled(label, label_style));
    }
    Line::from(spans)
}

/// The single global-key footer line. Pane-scoped actions (new/actions/collapse)
/// now live on the pane title bars, so the footer only lists keys that are global
/// regardless of focus.
const GLOBAL_HINT: &str =
    "[1-9/0 · ctrl+s n/p]tab · [ctrl+x/z]sub-tab · [?]help · [q]uit";

pub fn render(app: &App, c: &Computed, frame: &mut ratatui::Frame, area: Rect) {
    let p: &Palette = &c.palette;
    // Priority: armed prefix > searching > status line > selection-count > global.
    // The armed `ctrl+s` prefix takes the line so its awaiting-n/p state is obvious.
    if app.prefix_armed {
        frame.render_widget(
            Paragraph::new(hint_line(
                "prefix [ctrl+s] · [n] next tab · [p] prev tab · any other key cancels",
                p,
            )),
            area,
        );
        return;
    }
    let searching = c.searching.iter().any(|&s| s);
    if searching {
        frame.render_widget(
            Paragraph::new(hint_line("type to filter · [enter]apply · [esc]clear", p)),
            area,
        );
        return;
    }
    if let Some(status) = &app.status_line {
        frame.render_widget(
            Paragraph::new(Text::from(status.clone())).style(Style::default().fg(p.error)),
            area,
        );
        return;
    }
    // Selection count of the focused list pane — marks-aware, so this agrees
    // with the pane title bar it is describing (see `footer_bulk_count`).
    if let Some(count) = footer_bulk_count(c) {
        frame.render_widget(
            Paragraph::new(hint_line(
                // No verb hint here: which verbs accept a bulk selection is the
                // pane title bar's story (chips stay lit vs dim) — a footer `[a]`
                // hint went stale when `a` became the archive toggle.
                &format!("{count} selected · [shift+↑↓]extend · [esc]clear"),
                p,
            )),
            area,
        );
        return;
    }
    frame.render_widget(Paragraph::new(hint_line(GLOBAL_HINT, p)), area);
}

/// The focused list pane's effective bulk-selection count, or `None` when
/// there is nothing to show. Mirrors `selectors::pane_title`'s marks-aware
/// rule exactly (`is_bulk_selection` + `selected_positions`), so the footer
/// hint never disagrees with the pane title bar it is describing: a
/// marks-only selection (no anchored range) now reads as a selection here too,
/// and a mark that resolves to no visible row (filtered out by search) hides
/// the hint rather than showing a nonsensical "0 selected".
fn footer_bulk_count(c: &Computed) -> Option<usize> {
    let (count, bulk) = match c.ui.focus {
        PaneId::Queue => {
            let marks = &c.ui.marks[ListPane::Queue.idx()];
            let n = selected_positions(&c.queue, &c.queue_sel, marks, |r| r.task_id.clone()).len();
            (n, is_bulk_selection(&c.queue_sel, marks))
        }
        PaneId::Tasks => {
            let marks = &c.ui.marks[ListPane::Tasks.idx()];
            let n = selected_positions(&c.defs, &c.tasks_sel, marks, |d| {
                format!("{}/{}", d.repo, d.name)
            })
            .len();
            (n, is_bulk_selection(&c.tasks_sel, marks))
        }
        PaneId::Worktrees => {
            let marks = &c.ui.marks[ListPane::Worktrees.idx()];
            let n =
                selected_positions(&c.worktrees, &c.wt_sel, marks, |r| r.raw_name.clone()).len();
            (n, is_bulk_selection(&c.wt_sel, marks))
        }
        PaneId::Detail => return None,
    };
    (bulk && count > 0).then_some(count)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::marker::PhantomData;

    use crate::app::{Selection, TabUiState};
    use crate::selectors::WorktreeRow;

    use super::*;

    fn computed_with_worktrees(
        marks: HashSet<String>,
        wt_sel: Selection,
        worktrees: Vec<WorktreeRow>,
    ) -> Computed<'static> {
        let mut ui = TabUiState::default();
        ui.focus = PaneId::Worktrees;
        ui.marks[ListPane::Worktrees.idx()] = marks;
        Computed {
            palette: Palette::default(),
            active_name: None,
            tab_names: Vec::new(),
            active_index: 0,
            ui,
            queue: Vec::new(),
            defs: Vec::new(),
            worktrees,
            queue_sel: Selection::default(),
            tasks_sel: Selection::default(),
            wt_sel,
            searching: [false; 3],
            _marker: PhantomData,
        }
    }

    fn wrow(raw_name: &str) -> WorktreeRow {
        WorktreeRow { raw_name: raw_name.into(), ..Default::default() }
    }

    #[test]
    fn plain_cursor_no_marks_is_not_a_selection() {
        // No anchor, no marks — the cursor is just a viewport, matching
        // `pane_title`'s non-bulk case. Old footer behavior for a lone row.
        let c = computed_with_worktrees(
            HashSet::new(),
            Selection { cursor: 0, anchor: None },
            vec![wrow("w0")],
        );
        assert_eq!(footer_bulk_count(&c), None);
    }

    #[test]
    fn a_single_mark_counts_as_a_selection_like_the_pane_title_does() {
        // THE bug this fix closes: a lone Space-marked row is bulk (see
        // `selectors::pane_title_selection_count`'s same assertion) — the
        // footer must agree instead of reading the range alone.
        let c = computed_with_worktrees(
            HashSet::from(["w0".to_string()]),
            Selection { cursor: 5, anchor: None },
            vec![wrow("w0"), wrow("w1")],
        );
        assert_eq!(footer_bulk_count(&c), Some(1));
    }

    #[test]
    fn a_mark_that_resolves_to_no_visible_row_hides_the_hint() {
        // Mark present (bulk) but filtered out of the current view — mirrors
        // `pane_title_bulk_but_zero_resolved_hides_the_ghost_count`.
        let c = computed_with_worktrees(
            HashSet::from(["gone".to_string()]),
            Selection { cursor: 0, anchor: None },
            vec![wrow("w0")],
        );
        assert_eq!(footer_bulk_count(&c), None);
    }

    #[test]
    fn an_anchored_range_counts_normally() {
        let c = computed_with_worktrees(
            HashSet::new(),
            Selection { cursor: 2, anchor: Some(0) },
            vec![wrow("w0"), wrow("w1"), wrow("w2")],
        );
        assert_eq!(footer_bulk_count(&c), Some(3));
    }
}
