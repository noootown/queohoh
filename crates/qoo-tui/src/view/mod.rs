pub mod args_form;
pub mod def_args;
pub mod detail;
pub mod footer;
pub mod form;
pub mod help;
pub mod menu;
pub mod modal;
pub mod multiline_input;
pub mod panes;
pub mod settings;
pub mod tabbar;
pub mod theme;

use std::collections::HashSet;

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
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

pub(crate) fn clamp_sel(sel: &Selection, len: usize) -> Selection {
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
pub fn render(app: &mut App, frame: &mut ratatui::Frame) -> HitMap {
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

    // Backdrop: any overlay mutes the ENTIRE frame behind it so the popup reads
    // as the only live surface. Runs before the overlay renders — the overlay
    // Clears + redraws its own rect, so it stays at full color on top.
    if !matches!(app.mode, crate::app::Mode::List | crate::app::Mode::Search { .. }) {
        dim_backdrop(frame.buffer_mut(), &p);
    }

    // Overlays render last so their rects register topmost in the hit map.
    // `Mode::Form` is peeled off first via a `&mut` reborrow of `app.mode` (its
    // render needs `&mut FormState` to cache the focused Textarea's content
    // width — see `FormState::set_content_width`); every other overlay only
    // ever reads `app.mode`, so it stays on the original shared `match` in the
    // `else` branch (NLL ends the failed `if let`'s mutable borrow before it,
    // so `app.active_repo()` etc. below are unaffected).
    // The DefArgs prompt is resolved (and cloned) from `full_defs` BEFORE the
    // `&mut app.mode` render borrow below, so the render can hold `&mut state`
    // without also borrowing `app.full_defs`.
    let def_args_prompt: Option<String> = match &app.mode {
        crate::app::Mode::DefArgs { repo, def_name, .. } => {
            app.full_defs.get(&format!("{repo}/{def_name}")).map(|td| td.prompt.clone())
        }
        _ => None,
    };
    if let crate::app::Mode::Form { state, .. } = &mut app.mode {
        form::render_form(frame, &mut hits, state);
    } else if let crate::app::Mode::DefArgs { state, def_name, preview_scroll, .. } = &mut app.mode {
        let m = def_args::render_def_args(
            frame,
            &mut hits,
            &p,
            state,
            def_name,
            def_args_prompt.as_deref(),
            *preview_scroll,
        );
        app.menu_preview_max_scroll.set(m.max_scroll);
    } else {
        match &app.mode {
            crate::app::Mode::Help => help::render(frame, area, &mut hits, &p),
            crate::app::Mode::Settings => {
                settings::render(frame, area, &mut hits, &p, &app.settings)
            }
            crate::app::Mode::Confirm { title, body, confirm_label, focus, .. } => {
                modal::render_confirm(frame, &mut hits, title, body, confirm_label, *focus);
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
                // Render-feedback for wheel clamping (see the App fields).
                app.menu_preview_max_scroll.set(m.max_scroll);
            }
            crate::app::Mode::SessionPick { repo, worktree, items, loading, index, query, focus, .. } => {
                // Title is `{repo} · {worktree display name}`. The relative-age labels
                // read wall-clock now from `now_epoch_s` (→ ms).
                let title =
                    format!("{repo} · {}", crate::selectors::strip_repo_prefix(worktree, repo));
                let now_ms = app.now_epoch_s.saturating_mul(1000);
                let active_provider = app.active_provider().unwrap_or_default();
                menu::render_session_pick(
                    frame, &mut hits, &title, items, *loading, *index, query, now_ms, *focus,
                    &active_provider,
                );
            }
            crate::app::Mode::Form { .. } | crate::app::Mode::DefArgs { .. } => {
                unreachable!("handled by the `if let` chain above")
            }
            _ => {}
        }
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

/// The visible-row positions that make up the effective selection, ASCENDING and
/// deduplicated: the anchored range unioned with the marked rows.
///
/// The rule, stated once (every bulk consumer reads through this function):
///
/// - An **anchored range** contributes `[start, end]` inclusive.
/// - **Marks** contribute any row whose identity is in `marks`.
/// - The **cursor row** contributes ONLY in the degenerate case — no anchor and
///   no marks — where it is the whole selection.
///
/// That last clause is load-bearing. `selection_range` reports `(cursor, cursor)`
/// when there is no anchor, so a naive `range ∪ marks` would silently sweep the
/// cursor row into every marked selection: "mark row 0, move the cursor to row 3,
/// press `x`" would remove row 3 as well. Once the user has marked anything, the
/// cursor is just a viewport — not a selection.
///
/// With `marks` empty this reduces exactly to today's range behavior.
///
/// `sel` is clamped against `rows.len()` internally (same rule as [`clamp_sel`] /
/// `App::clamp_span`), so a daemon snapshot that shrinks the row set between the
/// selection and its use resolves to the surviving rows rather than panicking. A
/// mark whose identity matches no current row is silently dropped.
pub(crate) fn selected_positions<T>(
    rows: &[T],
    sel: &Selection,
    marks: &HashSet<String>,
    id_of: impl Fn(&T) -> String,
) -> Vec<usize> {
    if rows.is_empty() {
        return Vec::new();
    }
    let sel = clamp_sel(sel, rows.len());
    let has_anchor = sel.anchor.is_some();
    let (start, end) = selection_range(&sel);
    let mut out: Vec<usize> = (0..rows.len())
        .filter(|&pos| {
            let in_range = has_anchor && pos >= start && pos <= end;
            in_range || marks.contains(&id_of(&rows[pos]))
        })
        .collect();
    // Degenerate case: nothing anchored and nothing marked → the cursor row IS
    // the selection (today's single-target behavior).
    if out.is_empty() && !has_anchor && marks.is_empty() {
        out.push(sel.cursor);
    }
    out
}

/// Whether the pane's selection is a BULK one — a multi-row range or ANY mark.
/// Drives the not-applicable title-bar chip dimming
/// ([`crate::hit::bulk_allowed`] / `view::panes::button_chip`), the
/// status-line refusal in `App::bulk_blocked`, and the bulk-vs-single-target
/// branch in the `r`/`x` verbs.
///
/// A SINGLE mark counts as bulk: the bulk path resolves rows from `marks`, the
/// single-target path resolves them from the cursor, and with a mark present
/// those two disagree — so the mark must win, or `x` would act on a row the user
/// never marked.
///
/// Reads the UNCLAMPED `sel` on purpose (matching the historical `end > start`
/// on the raw `Selection`): when a snapshot shrinks the rows under a frozen
/// range, the action must still take the bulk path and let
/// [`selected_positions`] clamp to the survivors — clamping here first would
/// collapse the range to one row and silently reroute to the single-target
/// dispatch.
pub(crate) fn is_bulk_selection(sel: &Selection, marks: &HashSet<String>) -> bool {
    let (start, end) = selection_range(sel);
    end > start || !marks.is_empty()
}

/// Window `start` for a cursor-centered slice of `len` rows into `capacity`
/// rows. Uses only `window_rows(...).0` (see task assumption note): the first
/// returned value is the first-visible-row offset whether Task 5 returns
/// `(start, end_exclusive)` or `(offset, count)`.
pub(crate) fn window_start(len: usize, cursor: usize, capacity: usize) -> usize {
    crate::selectors::window_rows(len, cursor, capacity).0
}

/// Wrap the `width` glyph cells beginning at `(x, y)` in an [OSC 8] terminal
/// hyperlink to `url`. The visible glyphs are unchanged; the emitted byte
/// stream gains a zero-width opener before them and a closer after, so a
/// terminal that supports OSC 8 (Ghostty, iTerm2, kitty, WezTerm) turns the
/// chip into a real link. cmd+hover (pointer cursor) and cmd+click (open) are
/// the TERMINAL's job — handled natively even while mouse reporting is on,
/// because cmd bypasses the app's mouse capture and the cmd modifier is not
/// even representable in the xterm mouse protocol. The app therefore never sees
/// these clicks; a plain (no-cmd) click falls through to normal row selection.
///
/// Call this AFTER the cells are painted — it reads their symbols and rewrites
/// them. Technique: a terminal prints a cell's symbol verbatim, so the whole
/// link (opener + every glyph + closer) is folded into the FIRST cell's symbol
/// and the remaining glyph cells are `set_skip`ped. The explicit skips are
/// required because `Buffer::diff` only skips ONE cell after a wide symbol (its
/// `to_skip` resets each iteration); without them the later glyph cells would
/// re-print at the wrong column. The next real cell after the chip is
/// non-consecutive, so the backend re-anchors it with an absolute MoveTo and
/// the columns to the right are unaffected.
///
/// [OSC 8]: https://gist.github.com/egmontkob/eb114294efbcd5adb1944c9f3cb5feda
pub fn apply_osc8(buf: &mut Buffer, x: u16, y: u16, width: u16, url: &str) {
    if width == 0 {
        return;
    }
    // The glyphs already painted into the chip cells — wrap exactly what shows.
    let mut text = String::new();
    for i in 0..width {
        if let Some(cell) = buf.cell(Position { x: x.saturating_add(i), y }) {
            text.push_str(cell.symbol());
        }
    }
    if text.is_empty() {
        return;
    }
    // OSC 8 opener `ESC ] 8 ; ; URL ST`, the glyphs, then the closer
    // `ESC ] 8 ; ; ST`, where ST is `ESC \`. All folded into the first cell.
    let link = format!("\x1b]8;;{url}\x1b\\{text}\x1b]8;;\x1b\\");
    if let Some(cell) = buf.cell_mut(Position { x, y }) {
        cell.set_symbol(&link);
    }
    // Hide the remaining glyph cells so they are never re-emitted (see doc).
    for i in 1..width {
        if let Some(cell) = buf.cell_mut(Position { x: x.saturating_add(i), y }) {
            cell.set_skip(true);
        }
    }
}

/// Mute every cell of the frame so an overlay reads as the only live surface:
/// fg remapped to the palette's dim color, highlight bg dropped, and the
/// emphasis modifiers stripped. Deliberately an explicit color remap and NOT
/// the terminal `DIM` attribute (grey-on-grey is unreadable on this theme).
/// Style-only — symbols are never touched, so [`apply_osc8`]'s embedded
/// hyperlink escapes survive.
pub fn dim_backdrop(buf: &mut Buffer, p: &Palette) {
    use ratatui::style::{Color, Modifier};
    let area = buf.area;
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut(Position { x, y }) {
                cell.fg = p.dim;
                cell.bg = Color::Reset;
                cell.modifier
                    .remove(Modifier::BOLD | Modifier::REVERSED | Modifier::UNDERLINED);
            }
        }
    }
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
                hits = render(&mut app, frame);
            })
            .unwrap();
        (terminal, hits)
    }

    #[test]
    fn tabbar_shows_per_project_scheduled_plus_running_count() {
        // fixture: acme has 1 running + 1 queued task → chip suffix `(2)`.
        let app = fixture_app();
        let (terminal, _hits) = render_at(&app, 120, 40);
        let buf = terminal.backend().buffer().clone();
        let mut row0 = String::new();
        for x in 0..120 {
            row0.push_str(buf[(x, 0)].symbol());
        }
        assert!(row0.contains("1:acme (2)"), "tabbar row: {row0:?}");
    }

    #[test]
    fn header_shows_the_active_provider_indicator_and_registers_its_click_target() {
        // fixture snapshot's active provider is "grok" → the `↯ grok` indicator
        // renders at the header's right edge with a ProviderIndicator hit target.
        let app = fixture_app();
        let (terminal, hits) = render_at(&app, 120, 40);
        let buf = terminal.backend().buffer().clone();
        let mut row0 = String::new();
        for x in 0..120 {
            row0.push_str(buf[(x, 0)].symbol());
        }
        // ↯ is a plain width-1 glyph, so icon + single space + name is exact.
        assert!(row0.contains("↯ grok"), "no ↯ grok in header: {row0:?}");
        assert!(row0.trim_end().ends_with("grok"), "provider name missing at right edge: {row0:?}");
        // The indicator's rect is registered so a click opens the Switch form.
        assert!(
            hits.iter().any(|(_, t)| matches!(t, crate::hit::HitTarget::ProviderIndicator)),
            "no ProviderIndicator hit target registered"
        );
    }

    #[test]
    fn header_shows_usage_next_to_active_provider() {
        // Only grok enabled + matching usage → `↯ grok 42% mo` at the right edge.
        // ProviderIndicator hit covers the whole provider+usage cluster (click
        // cycles, same as `p`).
        use crate::ipc::types::{ProviderUsage, UsageSeverity};
        use ratatui::text::Span;

        let mut app = fixture_app();
        app.snapshot.as_mut().unwrap().enabled_providers = Some(vec!["grok".into()]);
        app.snapshot.as_mut().unwrap().provider_usages = Some(vec![ProviderUsage {
            provider: "grok".into(),
            text: "42% mo".into(),
            severity: UsageSeverity::Ok,
            fetched_at: 1,
            stale: false,
        }]);
        let (terminal, hits) = render_at(&app, 120, 40);
        let buf = terminal.backend().buffer().clone();
        let mut row0 = String::new();
        for x in 0..120 {
            row0.push_str(buf[(x, 0)].symbol());
        }
        assert!(
            row0.contains("↯ grok 42% mo"),
            "expected ↯ grok 42% mo in header: {row0:?}"
        );
        let trimmed = row0.trim_end();
        assert!(
            trimmed.ends_with("42% mo"),
            "usage should be rightmost in the cluster: {row0:?}"
        );

        let cluster_w = Span::raw("  ↯ grok 42% mo").width() as u16;
        let (rect, _) = hits
            .iter()
            .find(|(_, t)| matches!(t, HitTarget::ProviderIndicator))
            .expect("ProviderIndicator hit target registered");
        assert_eq!(rect.width, cluster_w, "hit covers full provider+usage cluster");
        assert_eq!(rect.x, 120 - cluster_w, "cluster is right-aligned");
    }

    #[test]
    fn header_shows_all_enabled_providers_with_usage() {
        // Snapshot lists claude + grok as enabled; both have samples; active
        // is grok. Both chips render; order follows enabled_providers. Codex
        // is absent from the list so it must not render even if a stale sample
        // arrived.
        use crate::ipc::types::{ProviderUsage, UsageSeverity};
        use ratatui::text::Span;

        let mut app = fixture_app();
        app.snapshot.as_mut().unwrap().enabled_providers =
            Some(vec!["claude".into(), "grok".into()]);
        app.snapshot.as_mut().unwrap().provider_usages = Some(vec![
            ProviderUsage {
                provider: "claude".into(),
                text: "10%".into(),
                severity: UsageSeverity::Ok,
                fetched_at: 1,
                stale: false,
            },
            ProviderUsage {
                provider: "grok".into(),
                text: "42% mo".into(),
                severity: UsageSeverity::Warn,
                fetched_at: 2,
                stale: false,
            },
            ProviderUsage {
                provider: "codex".into(),
                text: "n/a".into(),
                severity: UsageSeverity::Unknown,
                fetched_at: 3,
                stale: false,
            },
        ]);
        let (terminal, hits) = render_at(&app, 120, 40);
        let buf = terminal.backend().buffer().clone();
        let mut row0 = String::new();
        for x in 0..120 {
            row0.push_str(buf[(x, 0)].symbol());
        }
        assert!(
            row0.contains("↯ claude 10%"),
            "inactive claude+usage shown: {row0:?}"
        );
        assert!(
            row0.contains("↯ grok 42% mo"),
            "active grok+usage shown: {row0:?}"
        );
        assert!(
            !row0.contains("codex") && !row0.contains("n/a"),
            "disabled/unlisted provider must not render: {row0:?}"
        );
        // claude before grok (config precedence).
        let c = row0.find("↯ claude").expect("claude chip");
        let g = row0.find("↯ grok").expect("grok chip");
        assert!(c < g, "claude precedes grok: {row0:?}");

        let cluster_w = Span::raw("  ↯ claude 10%  ↯ grok 42% mo").width() as u16;
        let (rect, _) = hits
            .iter()
            .find(|(_, t)| matches!(t, HitTarget::ProviderIndicator))
            .expect("ProviderIndicator hit target registered");
        assert_eq!(rect.width, cluster_w);
        assert_eq!(rect.x, 120 - cluster_w);
    }

    #[test]
    fn header_omits_disabled_providers_even_when_settings_lists_them() {
        // Snapshot is authoritative: only grok enabled. Settings still lists
        // claude as enabled — must NOT re-introduce it.
        let mut app = fixture_app();
        app.snapshot.as_mut().unwrap().enabled_providers = Some(vec!["grok".into()]);
        app.snapshot.as_mut().unwrap().active_provider = Some("grok".into());
        app.settings = Some(Some(crate::ipc::types::SettingsPayload {
            active_provider: "grok".into(),
            providers: vec![
                crate::ipc::types::SettingsProvider {
                    name: "claude".into(),
                    enabled: true,
                    bin: None,
                },
                crate::ipc::types::SettingsProvider {
                    name: "grok".into(),
                    enabled: true,
                    bin: None,
                },
            ],
            ..Default::default()
        }));
        let (terminal, _hits) = render_at(&app, 120, 40);
        let buf = terminal.backend().buffer().clone();
        let mut row0 = String::new();
        for x in 0..120 {
            row0.push_str(buf[(x, 0)].symbol());
        }
        assert!(row0.contains("↯ grok"), "enabled grok shown: {row0:?}");
        assert!(!row0.contains("claude"), "snapshot-disabled claude omitted: {row0:?}");
    }

    #[test]
    fn header_hides_usage_when_sample_provider_not_shown() {
        // Only grok is enabled; single-chip sample is for claude → do not
        // show claude's chip or attach its text to the grok chip.
        use crate::ipc::types::{ProviderUsage, UsageSeverity};

        let mut app = fixture_app();
        app.snapshot.as_mut().unwrap().enabled_providers = Some(vec!["grok".into()]);
        app.snapshot.as_mut().unwrap().provider_usage = Some(ProviderUsage {
            provider: "claude".into(),
            text: "100%".into(),
            severity: UsageSeverity::Crit,
            fetched_at: 1,
            stale: false,
        });
        let (terminal, hits) = render_at(&app, 120, 40);
        let buf = terminal.backend().buffer().clone();
        let mut row0 = String::new();
        for x in 0..120 {
            row0.push_str(buf[(x, 0)].symbol());
        }
        assert!(row0.contains("↯ grok"), "provider still shown: {row0:?}");
        assert!(
            !row0.contains("100%") && !row0.contains("claude"),
            "unlisted provider/usage must not render: {row0:?}"
        );
        let prov_w = ratatui::text::Span::raw("  ↯ grok").width() as u16;
        let (rect, _) = hits
            .iter()
            .find(|(_, t)| matches!(t, HitTarget::ProviderIndicator))
            .expect("ProviderIndicator hit target registered");
        assert_eq!(rect.width, prov_w);
        assert_eq!(rect.x, 120 - prov_w);
    }

    #[test]
    fn header_omits_usage_when_none() {
        // provider_usages absent (fixture default): header ends with the provider
        // name, no percent/usage suffix after ↯ grok.
        let app = fixture_app();
        let (terminal, _hits) = render_at(&app, 120, 40);
        let buf = terminal.backend().buffer().clone();
        let mut row0 = String::new();
        for x in 0..120 {
            row0.push_str(buf[(x, 0)].symbol());
        }
        assert!(row0.contains("↯ grok"), "no ↯ grok in header: {row0:?}");
        assert!(
            row0.trim_end().ends_with("grok"),
            "without usage the header ends at the provider name: {row0:?}"
        );
        assert!(
            !row0.contains('%'),
            "no usage percent when provider_usages is None: {row0:?}"
        );
    }

    #[test]
    fn apply_osc8_folds_link_into_first_cell_and_skips_the_rest() {
        const URL: &str = "https://github.com/acme/acme/pull/77";
        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 1));
        // Paint "#77" starting at x=2, one glyph per cell.
        for (i, ch) in "#77".chars().enumerate() {
            buf[(2 + i as u16, 0)].set_symbol(&ch.to_string());
        }
        apply_osc8(&mut buf, 2, 0, 3, URL);
        // (a) first cell carries the full OSC 8 wrap around the glyphs.
        let s = buf[(2, 0)].symbol();
        assert!(s.starts_with(&format!("\x1b]8;;{URL}\x1b\\")), "opener present: {s:?}");
        assert!(s.contains("#77"), "glyphs preserved: {s:?}");
        assert!(s.ends_with("\x1b]8;;\x1b\\"), "closer present: {s:?}");
        // (b) the following two glyph cells are skipped so they never re-emit.
        assert!(buf[(3, 0)].skip, "second glyph cell skipped");
        assert!(buf[(4, 0)].skip, "third glyph cell skipped");
        // (c) width == 0 is inert — an untouched cell keeps its symbol.
        let before = buf[(6, 0)].symbol().to_string();
        apply_osc8(&mut buf, 6, 0, 0, URL);
        assert_eq!(buf[(6, 0)].symbol(), before, "zero width leaves the cell untouched");
    }

    #[test]
    fn dim_backdrop_mutes_styles_but_never_symbols() {
        use ratatui::style::{Color, Modifier, Style};
        let p = Palette::default();
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 1));
        buf[(0, 0)].set_symbol("x").set_style(
            Style::default().fg(Color::Green).bg(Color::Blue).add_modifier(Modifier::BOLD),
        );
        // An OSC 8-wrapped cell: the symbol must survive verbatim.
        buf[(1, 0)].set_symbol("\x1b]8;;https://x\x1b\\#7\x1b]8;;\x1b\\");
        buf[(1, 0)].modifier.insert(Modifier::UNDERLINED | Modifier::REVERSED);
        dim_backdrop(&mut buf, &p);
        assert_eq!(buf[(0, 0)].fg, p.dim);
        assert_eq!(buf[(0, 0)].bg, Color::Reset);
        assert!(!buf[(0, 0)].modifier.contains(Modifier::BOLD));
        assert!(!buf[(1, 0)].modifier.intersects(Modifier::UNDERLINED | Modifier::REVERSED));
        assert_eq!(buf[(1, 0)].symbol(), "\x1b]8;;https://x\x1b\\#7\x1b]8;;\x1b\\");
        assert_eq!(buf[(0, 0)].symbol(), "x");
    }

    #[test]
    fn discovering_def_swaps_the_marker_glyph_for_the_throbber() {
        // While a `d`-discover RPC is in flight the def row's `⌕` marker yields
        // to the animated throbber (queue running-row parity) — the visible
        // "search is happening" feedback between the keypress and the fan-out.
        let buffer_text = |t: &Terminal<TestBackend>| {
            let buf = t.backend().buffer();
            let area = *buf.area();
            let mut s = String::new();
            for y in 0..area.height {
                for x in 0..area.width {
                    s.push_str(buf[(x, y)].symbol());
                }
            }
            s
        };
        let mut app = fixture_app();
        app.defs_by_project.insert(
            "acme".to_string(),
            vec![crate::ipc::types::DefinitionSummary {
                repo: "acme".into(),
                name: "pr-review".into(),
                scope: "project".into(),
                has_discovery: true,
                ..Default::default()
            }],
        );
        let (idle, _) = render_at(&app, 120, 40);
        assert!(
            buffer_text(&idle).contains(crate::view::theme::GLYPH_DISCOVER),
            "idle discovery def shows the ⌕ marker"
        );
        app.discovering.insert("acme/pr-review".into());
        let (busy, _) = render_at(&app, 120, 40);
        assert!(
            !buffer_text(&busy).contains(crate::view::theme::GLYPH_DISCOVER),
            "an in-flight discover replaces ⌕ with the throbber"
        );
    }

    #[test]
    fn snapshot_default_80x24() {
        let (terminal, _hits) = render_at(&fixture_app(), 80, 24);
        insta::assert_snapshot!("view_default_80x24", terminal.backend());
    }

    #[test]
    fn snapshot_all_status_glyphs() {
        // One queue task per status pins the glyph set AND their WIDTHS: queued ○,
        // needs-input ‼, done ●, failed ✗, cancelled ⊘, skipped ⊝, verify-failed ⊗
        // (running uses the throbber). A glyph that renders double-width would
        // surface in a "Hidden by multi-width symbols" annotation and break column
        // alignment — this snapshot is the width check for the `‼`/`⊘`/`⊝`/`⊗`/`●`
        // glyphs.
        use crate::ipc::types::{Project, StateSnapshot, TaskInstance, TaskStatus};
        let mk = |id: &str, status: TaskStatus, created: &str| {
            let mut t = TaskInstance::default();
            t.id = id.into();
            t.status = status;
            t.target.repo = "acme".into();
            t.prompt = format!("{id} task");
            t.created = created.into();
            t
        };
        let mut app = fixture_app();
        app.snapshot = Some(StateSnapshot {
            tasks: vec![
                mk("que", TaskStatus::Queued, "2026-07-09T12:01:00.000Z"),
                mk("ipt", TaskStatus::NeedsInput, "2026-07-09T12:02:00.000Z"),
                mk("don", TaskStatus::Done, "2026-07-09T11:00:00.000Z"),
                mk("fai", TaskStatus::Failed, "2026-07-09T11:01:00.000Z"),
                mk("can", TaskStatus::Cancelled, "2026-07-09T11:02:00.000Z"),
                mk("skp", TaskStatus::Skipped, "2026-07-09T11:03:00.000Z"),
                mk("vrf", TaskStatus::VerifyFailed, "2026-07-09T11:04:00.000Z"),
            ],
            projects: vec![Project { name: "acme".into(), github_id: None }],
            ..Default::default()
        });
        let (terminal, _hits) = render_at(&app, 100, 24);
        insta::assert_snapshot!("view_all_status_glyphs", terminal.backend());
    }

    #[test]
    fn snapshot_wide_140x30() {
        // A wide terminal with a widened left column (override) so the pane inner
        // width clears the labeled-chip threshold: chips render as
        // `[s]chedule  [z]collapse` (queue) / `[c]ron` (tasks).
        let mut app = fixture_app();
        // Widened enough to clear the labeled-chip threshold AND leave the
        // WORKTREES pane room for the author + commit-age columns (they drop
        // first under the width ladder — before queued — so a too-narrow left
        // column hides them behind the busy row's queued·next cell). 102 (was
        // 100, was 98) — the `⛨` protected and `↣` merged front slots each
        // reserved 2 more cells.
        app.left_cols = Some(102);
        // Seed the TASKS pane with defs that exercise the schedule column:
        // discovery + humanized cron (cron text then the `⌕` marker), humanized
        // cron only (no marker), discovery with no cron (bare `⌕` marker), and a
        // plain def (blank schedule). Two carry a description so
        // the desc FILL column renders (blank on the two that don't); all four
        // carry a model so the model column renders (`provider/label` refs);
        // seeded here locally, not in the shared fixture, mirroring the
        // cron-column precedent.
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
                    cron_enabled: true,
                    description: Some("Review an open PR end to end.".into()),
                    model: Some("claude/claude-opus-4.8".into()),
                    worktree: None,
                },
                crate::ipc::types::DefinitionSummary {
                    repo: "acme".into(),
                    name: "nightly-tidy".into(),
                    scope: "project".into(),
                    cron: Some("0 2 * * *".into()),
                    // Paused: exercises the DIMMED schedule-column path (`[c]ron`
                    // toggled off). Not visible in the text-only snapshot — see the
                    // `sched_cell_style` unit test for the styling assertion.
                    cron_enabled: false,
                    description: Some("Nightly repo tidy sweep.".into()),
                    model: Some("claude/claude-sonnet-5".into()),
                    ..Default::default()
                },
                crate::ipc::types::DefinitionSummary {
                    repo: "acme".into(),
                    name: "deploy".into(),
                    scope: "project".into(),
                    has_discovery: true,
                    model: Some("claude/claude-fable-5".into()),
                    ..Default::default()
                },
                crate::ipc::types::DefinitionSummary {
                    repo: "acme".into(),
                    name: "lint".into(),
                    scope: "project".into(),
                    model: Some("claude/claude-haiku-4.5".into()),
                    ..Default::default()
                },
            ],
        );
        // Seed last-commit author + epoch on the acme worktrees so the WORKTREES
        // AUTHOR column renders (`koshea  3d ago` = who · when), and mark the
        // first worktree dirty + protected so the `±` and `⛨` front markers
        // render side by side. Local to this snapshot, not the shared fixture.
        if let Some(snap) = app.snapshot.as_mut()
            && let Some(wts) = snap.worktrees.get_mut("acme") {
                if let Some(w) = wts.get_mut(0) {
                    w.last_commit_author = Some("koshea".into());
                    w.last_commit_epoch = Some(app.now_epoch_s - 3 * 86_400);
                    w.dirty = Some(true);
                    w.protected = true;
                    // …and merged, so all three front markers render side by side.
                    w.merged = Some(true);
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
            CatalogEntry, DefaultModels, DefaultModelsProject, SettingsPayload, SettingsProvider,
        };
        let prov = |name: &str, enabled: bool| SettingsProvider { name: name.into(), enabled, bin: None };
        let cat = |provider: &str, id: &str, label: &str, hidden: bool| CatalogEntry {
            provider: provider.into(),
            id: id.into(),
            label: label.into(),
            hidden,
        };
        let mut app = fixture_app();
        app.mode = crate::app::Mode::Settings;
        // Some(Some(_)): the post-Task-5 shape — providers (active marked), a
        // merged catalog (hidden marked), and default-model chains (global +
        // one project override) — exercising every row kind the overlay renders.
        app.settings = Some(Some(SettingsPayload {
            active_provider: "grok".into(),
            providers: vec![prov("claude", true), prov("grok", true), prov("codex", false)],
            catalog: vec![
                cat("claude", "claude-opus-4-8", "claude-opus-4.8", false),
                cat("grok", "grok-4.5", "grok-4.5", false),
                cat("grok", "grok-legacy", "legacy", true),
            ],
            default_models: DefaultModels {
                global: vec!["claude/claude-opus-4.8".into(), "grok/grok-4.5".into()],
                projects: vec![DefaultModelsProject {
                    name: "acme".into(),
                    default_models: vec!["grok/grok-4.5".into()],
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

    #[test]
    fn worktrees_pane_pr_cell_is_an_osc8_link() {
        // A worktree with BOTH a PR number and its url gets its `#<n>` PR cell
        // wrapped in an OSC 8 terminal hyperlink (the app no longer registers a
        // click target for it — the terminal handles cmd+click). A wide left
        // column keeps the PR column past the width ladder.
        let mut app = fixture_app();
        let url = "https://github.com/acme/acme/pull/77".to_string();
        if let Some(w) = app
            .snapshot
            .as_mut()
            .and_then(|s| s.worktrees.get_mut("acme"))
            .and_then(|wts| wts.iter_mut().find(|w| w.name == "acme.feature"))
        {
            w.pr_number = Some(77);
            w.pr_url = Some(url.clone());
        }
        app.left_cols = Some(110);
        let (terminal, hits) = render_at(&app, 140, 30);
        let buf = terminal.backend().buffer();
        let opener = format!("\x1b]8;;{url}\x1b\\");
        // Exactly one cell carries the OSC 8 link (folded into the first glyph).
        let mut found: Option<(u16, u16)> = None;
        let mut count = 0usize;
        for y in buf.area.y..buf.area.bottom() {
            for x in buf.area.x..buf.area.right() {
                if buf[(x, y)].symbol().contains(&opener) {
                    count += 1;
                    found = Some((x, y));
                }
            }
        }
        assert_eq!(count, 1, "exactly one OSC 8 link cell in the pane");
        let (x, y) = found.expect("OSC 8 link cell present");
        let sym = buf[(x, y)].symbol();
        assert!(sym.contains("#77"), "the wrapped glyphs are #77: {sym:?}");
        assert!(sym.ends_with("\x1b]8;;\x1b\\"), "closer present: {sym:?}");
        // The link affordance (underline) is preserved on the first cell.
        assert!(
            buf[(x, y)].modifier.contains(ratatui::style::Modifier::UNDERLINED),
            "the #77 link cell is underlined"
        );
        // A plain (no-cmd) click falls through to row selection: a Worktrees Row
        // hit still covers that cell.
        assert!(
            hits.iter().any(|(rect, t)| matches!(
                t,
                HitTarget::Row(crate::app::ListPane::Worktrees, _)
            ) && rect.contains(ratatui::layout::Position { x, y })),
            "a Worktrees Row hit covers the PR cell so a plain click selects the row"
        );
    }

    #[test]
    fn selected_def_row_without_cron_paints_full_width_selection() {
        // A def with no cron/description/model → `def_line` appends nothing past
        // the name, so its spans end early. Selecting it must STILL paint the
        // selection bg across the whole row (the fix pads the line to the row
        // width before patching); otherwise the tail cells stay unhighlighted.
        let mut app = fixture_app();
        app.defs_by_project.insert(
            "acme".to_string(),
            vec![crate::ipc::types::DefinitionSummary {
                repo: "acme".into(),
                name: "lint".into(),
                scope: "project".into(),
                ..Default::default()
            }],
        );
        let mut ui = crate::app::TabUiState::default();
        ui.focus = PaneId::Tasks;
        ui.last_list_pane = crate::app::ListPane::Tasks;
        ui.selections[crate::app::ListPane::Tasks as usize].cursor = 0;
        app.ui_by_tab.insert("acme".to_string(), ui);

        let (terminal, _hits) = render_at(&app, 80, 24);
        let buf = terminal.backend().buffer();
        let sel_bg = crate::view::theme::Palette::default().selection_bg;
        // Find the row that renders the "lint" def and count its selection-bg
        // cells: the selected short-def row extends far past its 4-char name.
        let mut sel_count = 0usize;
        for y in 0..buf.area.height {
            let row: String =
                (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect();
            if row.contains("lint") {
                let count =
                    (0..buf.area.width).filter(|&x| buf[(x, y)].bg == sel_bg).count();
                sel_count = sel_count.max(count);
            }
        }
        assert!(
            sel_count >= 20,
            "selection bg spans well past the short def name (got {sel_count} cells)"
        );
    }
}

#[cfg(test)]
mod selection_tests {
    use super::*;
    use std::collections::HashSet;

    /// Rows are their own identity — keeps the tests about the selection rule,
    /// not about identity extraction.
    fn rows(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("r{i}")).collect()
    }
    fn marks_of(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }
    fn positions(rows: &[String], sel: Selection, marks: &HashSet<String>) -> Vec<usize> {
        selected_positions(rows, &sel, marks, |r| r.clone())
    }

    #[test]
    fn no_anchor_no_marks_selects_only_the_cursor_row() {
        // The degenerate case — exactly today's single-target behavior.
        let r = rows(5);
        let sel = Selection { cursor: 2, anchor: None };
        assert_eq!(positions(&r, sel, &HashSet::new()), vec![2]);
    }

    #[test]
    fn an_anchor_selects_the_inclusive_range() {
        let r = rows(5);
        let sel = Selection { cursor: 3, anchor: Some(1) };
        assert_eq!(positions(&r, sel, &HashSet::new()), vec![1, 2, 3]);
    }

    #[test]
    fn marks_alone_select_exactly_the_marked_rows_not_the_cursor() {
        // THE load-bearing rule: with marks present and no anchor, a cursor
        // parked on an unmarked row must NOT be swept into the selection.
        // Without this, "mark row 0, move to row 3, press x" would destroy row 3.
        let r = rows(5);
        let sel = Selection { cursor: 3, anchor: None };
        assert_eq!(positions(&r, sel, &marks_of(&["r0"])), vec![0]);
    }

    #[test]
    fn range_and_marks_union_in_ascending_order_without_duplicates() {
        // Range [2..=3] plus marks on r0 and r3 (r3 overlaps the range).
        let r = rows(6);
        let sel = Selection { cursor: 3, anchor: Some(2) };
        assert_eq!(positions(&r, sel, &marks_of(&["r0", "r3"])), vec![0, 2, 3]);
    }

    #[test]
    fn a_stale_mark_is_silently_excluded() {
        // "r9" isn't in the current rows (removed by another session / filtered
        // out of the snapshot) — it must resolve to nothing, not panic. It must
        // NOT fall back to the cursor row either: that fallback is reserved for
        // the true degenerate case (no anchor AND no marks at all). A present
        // but stale mark still means "the user has marked something," so the
        // cursor stays a pure viewport — exactly the load-bearing rule this
        // task exists to enforce (see `marks_alone_select_exactly_the_marked_
        // rows_not_the_cursor` above).
        let r = rows(3);
        let sel = Selection { cursor: 0, anchor: None };
        assert_eq!(positions(&r, sel, &marks_of(&["r9"])), Vec::<usize>::new());
    }

    #[test]
    fn positions_clamp_against_a_shrunken_row_set() {
        // Race: the range was anchored at 3..=5, then the visible rows shrank to
        // 2. Clamping (mirroring `clamp_sel`) yields the surviving row, matching
        // what `clamp_span` does for the existing bulk paths.
        let r = rows(2);
        let sel = Selection { cursor: 5, anchor: Some(3) };
        assert_eq!(positions(&r, sel, &HashSet::new()), vec![1]);
    }

    #[test]
    fn empty_rows_select_nothing() {
        let r: Vec<String> = vec![];
        let sel = Selection { cursor: 0, anchor: None };
        assert!(positions(&r, sel, &marks_of(&["r0"])).is_empty());
    }

    #[test]
    fn is_bulk_is_true_for_a_range_or_any_mark() {
        let plain = Selection { cursor: 2, anchor: None };
        let ranged = Selection { cursor: 3, anchor: Some(1) };
        assert!(!is_bulk_selection(&plain, &HashSet::new()));
        assert!(is_bulk_selection(&ranged, &HashSet::new()));
        // A SINGLE mark is still a bulk selection: it must route through the
        // bulk path (which reads marks) rather than the single-target path
        // (which reads the cursor row) — otherwise the two would disagree.
        assert!(is_bulk_selection(&plain, &marks_of(&["r0"])));
    }

    #[test]
    fn is_bulk_reads_the_unclamped_selection() {
        // Deliberately NOT clamped: the shrink race (range 3..=5 over 2 rows)
        // must still report bulk so the caller takes the RpcSeq path, matching
        // `queue_range_requeue_clamps_when_rows_shrink_below_frozen_start`.
        let sel = Selection { cursor: 5, anchor: Some(3) };
        assert!(is_bulk_selection(&sel, &HashSet::new()));
    }
}
