pub mod args_form;
pub mod detail;
pub mod footer;
pub mod help;
pub mod menu;
pub mod modal;
pub mod panes;
pub mod settings;
pub mod tabbar;
pub mod theme;

use ratatui::layout::{Constraint, Direction, Layout, Rect};
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

    // Default: Percentage(34) split (byte-identical to the pre-drag layout).
    // Once the vertical divider has been dragged, an absolute Length override
    // (clamped so neither side collapses) drives the split instead.
    let cols = match app.left_cols {
        Some(n) => {
            let w = crate::selectors::clamp_left_cols(body.width, n);
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(w), Constraint::Min(1)])
                .split(body)
        }
        None => Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(34), Constraint::Min(1)])
            .split(body),
    };
    let (left, right) = (cols[0], cols[1]);

    panes::render(app, &c, frame, left, &mut hits);
    detail::render(app, &c, frame, right, &mut hits);
    footer::render(app, &c, frame, foot);

    // Draggable vertical divider: the two adjacent border columns between the
    // left pane stack (its right border at left.right()−1) and DETAIL (its left
    // border at left.right()). Registered after both regions so it wins the
    // reverse hit scan on those columns; the left panes' scrollbar track sits one
    // column further in (left.right()−2), so there is no overlap.
    hits.push(
        Rect {
            x: left.right().saturating_sub(1),
            y: body.y,
            width: 2,
            height: body.height,
        },
        crate::hit::HitTarget::PaneDividerV,
    );

    // Overlays render last so their rects register topmost in the hit map.
    match &app.mode {
        crate::app::Mode::Help => help::render(frame, area, &mut hits, &p),
        crate::app::Mode::Settings => {
            settings::render(frame, area, &mut hits, &p, &app.settings)
        }
        crate::app::Mode::ActionMenu { title, items, index, query, preview_scroll } => {
            let state =
                menu::PickerState { index: *index, query, preview_scroll: *preview_scroll };
            let m = menu::render_menu(frame, &mut hits, title, items, state);
            // Render-feedback for ctrl+d/u + wheel clamping (see the App fields).
            app.menu_preview_max_scroll.set(m.max_scroll);
            app.menu_preview_page.set(m.half_page);
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
        crate::app::Mode::DefPick { defs, index, worktree, branch, query, preview_scroll } => {
            let repo = app.active_repo().unwrap_or_default();
            let title = match worktree {
                Some(wt) => {
                    format!("Tasks — {}:{}", repo, crate::selectors::strip_repo_prefix(wt, &repo))
                }
                None => format!("Tasks — {repo}"),
            };
            let _ = branch;
            // Resolve the highlighted (filtered) def's full prompt for the right
            // pane: filter by name, map the display index to the underlying def,
            // then look it up in `full_defs` keyed "repo/name".
            let filtered = crate::selectors::filter_rows(defs, query, |d| d.name.clone());
            let full = filtered
                .get(*index)
                .and_then(|&i| defs.get(i))
                .and_then(|d| app.full_defs.get(&format!("{}/{}", d.repo, d.name)));
            let state =
                menu::PickerState { index: *index, query, preview_scroll: *preview_scroll };
            let m = menu::render_def_pick(frame, &mut hits, &title, defs, full, state);
            // Render-feedback for ctrl+d/u + wheel clamping (see the App fields).
            app.menu_preview_max_scroll.set(m.max_scroll);
            app.menu_preview_page.set(m.half_page);
        }
        crate::app::Mode::DefArgs { form } => {
            // Resolve the def's full prompt for the right panel (keyed "repo/name",
            // same source as the DefPick preview); `None` until the fetch lands.
            let full = app.full_defs.get(&format!("{}/{}", form.repo, form.def_name));
            let m = args_form::render_run_form(frame, &mut hits, &p, form, full);
            // Render-feedback for ctrl+d/u + wheel clamping (see the App fields).
            app.menu_preview_max_scroll.set(m.max_scroll);
            app.menu_preview_page.set(m.half_page);
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
    fn snapshot_wide_140x30() {
        // A wide terminal with a widened left column (override) so the pane inner
        // width clears the labeled-chip threshold: chips render as
        // `[c]reate  [a]ctions  [z] collapse`.
        let mut app = fixture_app();
        // Widened enough to clear the labeled-chip threshold AND leave the
        // WORKTREES pane room for the author + commit-age columns (they drop
        // first under the width ladder — before queued — so a too-narrow left
        // column hides them behind the busy row's queued·next cell).
        app.left_cols = Some(98);
        // Seed the TASKS pane with defs that exercise the schedule column:
        // discovery + humanized cron, humanized cron only, a bare ⏰ (discovery,
        // no cron), and a plain def (blank schedule). Two carry a description so
        // the desc FILL column renders (blank on the two that don't); all four
        // carry a model so the model column renders (two `claude-`-prefixed to
        // exercise stripping, two plain aliases); seeded here locally, not in the
        // shared fixture, mirroring the cron-column precedent.
        app.defs_by_project.insert(
            "acme".to_string(),
            vec![
                crate::ipc::types::DefinitionSummary {
                    repo: "acme".into(),
                    name: "pr-review".into(),
                    scope: "project".into(),
                    args: vec![crate::ipc::types::ArgSpec { name: "pr".into(), ..Default::default() }],
                    has_discovery: true,
                    cron: Some("30 13 * * *".into()),
                    description: Some("Review an open PR end to end.".into()),
                    model: Some("claude-opus-4-8".into()),
                },
                crate::ipc::types::DefinitionSummary {
                    repo: "acme".into(),
                    name: "nightly-tidy".into(),
                    scope: "project".into(),
                    cron: Some("0 2 * * *".into()),
                    description: Some("Nightly repo tidy sweep.".into()),
                    model: Some("sonnet".into()),
                    ..Default::default()
                },
                crate::ipc::types::DefinitionSummary {
                    repo: "acme".into(),
                    name: "deploy".into(),
                    scope: "project".into(),
                    has_discovery: true,
                    model: Some("claude-fable-5".into()),
                    ..Default::default()
                },
                crate::ipc::types::DefinitionSummary {
                    repo: "acme".into(),
                    name: "lint".into(),
                    scope: "project".into(),
                    model: Some("haiku".into()),
                    ..Default::default()
                },
            ],
        );
        // Seed last-commit author + epoch on the acme worktrees so the WORKTREES
        // AUTHOR column renders (`koshea  3d ago` = who · when). Local to this
        // snapshot, not the shared fixture.
        if let Some(snap) = app.snapshot.as_mut()
            && let Some(wts) = snap.worktrees.get_mut("acme") {
                if let Some(w) = wts.get_mut(0) {
                    w.last_commit_author = Some("koshea".into());
                    w.last_commit_epoch = Some(app.now_epoch_s - 3 * 86_400);
                }
                if let Some(w) = wts.get_mut(1) {
                    w.last_commit_author = Some("ada".into());
                    w.last_commit_epoch = Some(app.now_epoch_s - 6 * 3600);
                }
            }
        let (terminal, _hits) = render_at(&app, 140, 30);
        insta::assert_snapshot!("view_wide_140x30", terminal.backend());
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
    fn snapshot_settings_overlay() {
        use crate::ipc::types::{
            SettingsLayer, SettingsModels, SettingsPayload, SettingsProjectLayer,
        };
        use std::collections::BTreeMap;
        let m = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
            pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
        };
        let mut app = fixture_app();
        app.mode = crate::app::Mode::Settings;
        // Some(Some(_)): defaults ⊕ a global remap (sonnet) + one project delta,
        // exercising all three row kinds the overlay renders.
        app.settings = Some(Some(SettingsPayload {
            models: SettingsModels {
                defaults: m(&[
                    ("haiku", "claude-haiku-4-5"),
                    ("opus", "claude-opus-4-8"),
                    ("sonnet", "claude-sonnet-4-5"),
                ]),
                global: SettingsLayer {
                    entries: m(&[("sonnet", "claude-sonnet-4-6")]),
                    source: "~/.config/qoo/config.yaml".into(),
                },
                projects: vec![SettingsProjectLayer {
                    repo: "acme".into(),
                    entries: m(&[("opus", "claude-opus-4-9")]),
                    source: "acme/vars.yaml".into(),
                }],
            },
        }));
        let (terminal, hits) = render_at(&app, 80, 24);
        insta::assert_snapshot!("view_settings_overlay", terminal.backend());
        assert!(
            hits.iter().any(|(_, t)| *t == HitTarget::Modal),
            "settings overlay registers a topmost Modal hit target"
        );
    }

    #[test]
    fn snapshot_collapsed_queue_and_tasks() {
        // Collapse the top two panes: each renders only its 2-row title bar and
        // worktrees expands to fill the freed height.
        let mut app = fixture_app();
        app.collapsed = [true, true, false];
        let (terminal, hits) = render_at(&app, 80, 24);
        insta::assert_snapshot!("view_collapsed_queue_tasks", terminal.backend());
        // Collapsed bars keep their expand chip clickable (no whole-row toggle —
        // that target swallowed divider drags and was removed).
        let chips = hits
            .iter()
            .filter(|(_, t)| {
                matches!(t, HitTarget::PaneButton(_, crate::hit::PaneButton::Collapse))
            })
            .count();
        assert!(chips >= 2, "collapsed panes keep expand chips (got {chips})");
        // The collapsed queue pane registers no Row/PaneBody hits.
        assert!(
            !hits.iter().any(|(_, t)| matches!(t, HitTarget::Row(crate::app::ListPane::Queue, _))),
            "collapsed queue has no row hit targets"
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
