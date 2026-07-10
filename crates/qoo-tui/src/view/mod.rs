pub mod args_form;
pub mod detail;
pub mod footer;
pub mod help;
pub mod menu;
pub mod modal;
pub mod panes;
pub mod tabbar;
pub mod theme;

use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::Style;
use ratatui::text::Text;
use ratatui::widgets::Paragraph;

use crate::app::{App, Selection, TabUiState};
use crate::hit::HitMap;
use crate::ipc::types::DefinitionSummary;
use crate::selectors::{
    QueueRow, WorktreeRow, build_tabs, filter_rows, queue_rows, worktree_rows,
};
use theme::Palette;

/// Everything a frame needs, computed once so hit-testing and drawing use the
/// same geometry and the same filtered/selected view-model.
pub struct Computed<'a> {
    pub palette: Palette,
    pub active_name: Option<String>,
    pub tab_names: Vec<String>,
    pub active_index: usize,
    pub ui: TabUiState,
    pub queue: Vec<QueueRow>,
    pub defs: Vec<DefinitionSummary>,
    pub worktrees: Vec<WorktreeRow>,
    pub queue_sel: Selection,
    pub tasks_sel: Selection,
    pub wt_sel: Selection,
    /// Per-list-pane search-mode flag (`[queue, tasks, worktrees]`). All false
    /// until Task 11 adds `Mode::Search`; wired here so panes/footer read it
    /// without pattern-matching a not-yet-existent variant.
    pub searching: [bool; 3],
    pub _marker: std::marker::PhantomData<&'a ()>,
}

fn clamp_sel(sel: &Selection, len: usize) -> Selection {
    if len == 0 {
        return Selection { cursor: 0, anchor: None };
    }
    let cursor = sel.cursor.min(len - 1);
    let anchor = sel.anchor.map(|a| a.min(len - 1));
    Selection { cursor, anchor }
}

/// The compute pass. Derives the active project, its filtered rows, and clamped
/// selections. Pure — no drawing.
pub fn compute(app: &App) -> Computed<'_> {
    let palette = Palette::default();
    let tabs = app
        .snapshot
        .as_ref()
        .map(build_tabs)
        .unwrap_or_default();
    let active_index = app.active_tab.min(tabs.len().saturating_sub(1));
    let active_name = tabs.get(active_index).map(|t| t.name.clone());
    let ui = active_name
        .as_ref()
        .and_then(|n| app.ui_by_tab.get(n).cloned())
        .unwrap_or_default();

    let (queue, defs, worktrees) = match (&app.snapshot, &active_name) {
        (Some(snap), Some(name)) => {
            let q = queue_rows(snap, name, app.now_epoch_s);
            let d = app.defs_by_project.get(name).cloned().unwrap_or_default();
            let w = worktree_rows(snap, name);
            (q, d, w)
        }
        _ => (Vec::new(), Vec::new(), Vec::new()),
    };

    // Filter each pane by its search string (indices → owned rows).
    let q_idx = filter_rows(&queue, &ui.search[0], |r| r.summary.clone());
    let d_idx = filter_rows(&defs, &ui.search[1], |d| d.name.clone());
    let w_idx = filter_rows(&worktrees, &ui.search[2], |r| r.name.clone());
    let queue: Vec<QueueRow> = q_idx.into_iter().map(|i| queue[i].clone()).collect();
    let defs: Vec<DefinitionSummary> = d_idx.into_iter().map(|i| defs[i].clone()).collect();
    let worktrees: Vec<WorktreeRow> = w_idx.into_iter().map(|i| worktrees[i].clone()).collect();

    let queue_sel = clamp_sel(&ui.selections[0], queue.len());
    let tasks_sel = clamp_sel(&ui.selections[1], defs.len());
    let wt_sel = clamp_sel(&ui.selections[2], worktrees.len());

    // Search-mode projection: the one pane being typed into shows its cursor
    // block (`pane_title`) and the searching footer hint. All false otherwise.
    let mut searching = [false; 3];
    if let crate::app::Mode::Search { pane } = &app.mode {
        searching[*pane as usize] = true;
    }

    Computed {
        palette,
        active_name,
        tab_names: tabs.iter().map(|t| t.name.clone()).collect(),
        active_index,
        ui,
        queue,
        defs,
        worktrees,
        queue_sel,
        tasks_sel,
        wt_sel,
        searching,
        _marker: std::marker::PhantomData,
    }
}

/// Draw the whole frame, returning the hit map for mouse routing.
pub fn render(app: &App, frame: &mut ratatui::Frame) -> HitMap {
    let mut hits = HitMap::new();
    let area = frame.area();
    let p = Palette::default();

    if area.width < 60 || area.height < 15 {
        let msg = Paragraph::new(Text::from("terminal too small (60x15 minimum)"))
            .style(Style::default().fg(p.fg));
        frame.render_widget(msg, area);
        return hits; // no clickable targets while too small
    }

    let c = compute(app);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Min(1),    // body
            Constraint::Length(1), // footer
        ])
        .split(area);
    let (header, body, foot) = (rows[0], rows[1], rows[2]);

    tabbar::render(app, &c, frame, header, &mut hits);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(34), Constraint::Min(1)])
        .split(body);
    let (left, right) = (cols[0], cols[1]);

    panes::render(app, &c, frame, left, &mut hits);
    detail::render(app, &c, frame, right, &mut hits);
    footer::render(app, &c, frame, foot);

    // Overlays render last so their rects register topmost in the hit map.
    match &app.mode {
        crate::app::Mode::Help => help::render(frame, area, &mut hits, &p),
        crate::app::Mode::ActionMenu { title, items, index } => {
            menu::render_menu(frame, &mut hits, title, items, *index);
        }
        crate::app::Mode::ConfirmRemove { worktree, branch, .. } => {
            menu::render_confirm_remove(frame, &mut hits, worktree, branch);
        }
        crate::app::Mode::ConfirmBulkRemove { names, .. } => {
            modal::render_confirm_bulk_remove(frame, &mut hits, names);
        }
        crate::app::Mode::AddTask { worktree, session, input } => {
            let repo = app.active_repo().unwrap_or_default();
            let target = match worktree {
                Some(w) => format!("{repo}:{}", crate::selectors::strip_repo_prefix(w, &repo)),
                None => format!("{repo} (adhoc)"),
            };
            let sess = match session {
                crate::app::SessionMode::Fresh => "fresh",
                crate::app::SessionMode::Main => "main",
            };
            modal::render_input_modal(
                frame,
                &mut hits,
                &format!("New task — {sess} session — {target}"),
                "prompt",
                input,
            );
        }
        crate::app::Mode::WorktreeInput { task_id, input } => {
            let last6: String = task_id.chars().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect();
            modal::render_input_modal(
                frame,
                &mut hits,
                &format!("Assign worktree — task {last6}"),
                "worktree",
                input,
            );
        }
        crate::app::Mode::DefPick { defs, index, worktree, branch } => {
            let repo = app.active_repo().unwrap_or_default();
            let title = match worktree {
                Some(wt) => format!(
                    "Run task definition — {}:{}",
                    repo,
                    crate::selectors::strip_repo_prefix(wt, &repo)
                ),
                None => format!("Run task definition — {repo}"),
            };
            let _ = branch;
            menu::render_def_pick(frame, &mut hits, &title, defs, *index);
        }
        crate::app::Mode::DefArgs { form } => {
            args_form::render_args_form(frame, &mut hits, &p, form);
        }
        crate::app::Mode::CreateWorktree { input, error } => {
            let repo = app.active_repo().unwrap_or_default();
            modal::render_create_worktree(frame, &mut hits, &repo, input, error.as_deref());
        }
        _ => {}
    }

    hits
}

/// Inclusive `(start, end)` selection range from a `Selection`.
pub(crate) fn selection_range(sel: &Selection) -> (usize, usize) {
    match sel.anchor {
        Some(a) => (a.min(sel.cursor), a.max(sel.cursor)),
        None => (sel.cursor, sel.cursor),
    }
}

/// Window `start` for a cursor-centered slice of `len` rows into `capacity`
/// rows. Uses only `window_rows(...).0` (see task assumption note): the first
/// returned value is the first-visible-row offset whether Task 5 returns
/// `(start, end_exclusive)` or `(offset, count)`.
pub(crate) fn window_start(len: usize, cursor: usize, capacity: usize) -> usize {
    crate::selectors::window_rows(len, cursor, capacity).0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::PaneId;
    use crate::hit::HitTarget;
    use crate::test_fixtures::fixture_app;
    use ratatui::{Terminal, backend::TestBackend};

    fn render_at(app: &App, w: u16, h: u16) -> (Terminal<TestBackend>, HitMap) {
        let mut app = app.clone();
        app.size = (w, h);
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        let mut hits = HitMap::new();
        terminal
            .draw(|frame| {
                hits = render(&app, frame);
            })
            .unwrap();
        (terminal, hits)
    }

    #[test]
    fn snapshot_default_80x24() {
        let (terminal, _hits) = render_at(&fixture_app(), 80, 24);
        insta::assert_snapshot!("view_default_80x24", terminal.backend());
    }

    #[test]
    fn snapshot_too_small() {
        let (terminal, hits) = render_at(&fixture_app(), 40, 10);
        insta::assert_snapshot!("view_too_small", terminal.backend());
        assert!(hits.is_empty(), "too-small guard registers no hit targets");
    }

    #[test]
    fn snapshot_help_overlay() {
        let mut app = fixture_app();
        app.mode = crate::app::Mode::Help;
        let (terminal, hits) = render_at(&app, 80, 24);
        insta::assert_snapshot!("view_help_overlay", terminal.backend());
        assert!(
            hits.iter().any(|(_, t)| *t == HitTarget::Modal),
            "help overlay registers a topmost Modal hit target"
        );
    }

    #[test]
    fn snapshot_disconnected() {
        let mut app = fixture_app();
        app.connected = false;
        let (terminal, _hits) = render_at(&app, 80, 24);
        insta::assert_snapshot!("view_disconnected", terminal.backend());
    }

    #[test]
    fn hitmap_has_one_tab_target() {
        let (_t, hits) = render_at(&fixture_app(), 80, 24);
        let tabs = hits
            .iter()
            .filter(|(_, t)| matches!(t, HitTarget::Tab(_)))
            .count();
        assert_eq!(tabs, 1, "fixture has one project → one clickable tab");
    }

    #[test]
    fn hitmap_has_queue_rows_and_bodies() {
        let (_t, hits) = render_at(&fixture_app(), 80, 24);
        let rows = hits
            .iter()
            .filter(|(_, t)| matches!(t, HitTarget::Row(crate::app::ListPane::Queue, _)))
            .count();
        assert!(rows >= 3, "3 live + 1 archived queue rows visible");
        assert!(
            hits.iter().any(|(_, t)| *t == HitTarget::PaneBody(PaneId::Queue)),
            "queue pane body registered for empty-area clicks"
        );
    }
}
