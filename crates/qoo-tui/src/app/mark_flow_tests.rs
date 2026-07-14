use super::*;
use crate::ipc::types::{Project, StateSnapshot, TaskInstance, TaskStatus, WorktreeInfo};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::{HashMap, HashSet};

fn key(c: char) -> Event { Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)) }
fn space() -> Event { Event::Key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)) }
fn down() -> Event { Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)) }
fn tab() -> Event { Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)) }

fn app_with(snap: StateSnapshot) -> App {
    let mut a = App::new("/tmp/runs".into(), "/tmp/s.sock".into());
    a.size = (120, 40);
    // Isolate marks tests from the self-heal effect: a test snapshot's
    // `build_id` is always `None`, which `heal_on_snapshot` treats as always
    // stale against a locally-built (gitignored) daemon dist, setting a status
    // line that would otherwise bleed into the very next keypress's `dirty`
    // flag (see `update.rs`'s `had_status` merge). Unrelated to marks; same
    // mitigation as `heal_wiring_tests.rs`'s attach-only-mode tests.
    a.heal_enabled = false;
    a.update(Event::Snapshot(snap));
    a
}

/// Three queued tasks on `platform`, ids t0/t1/t2 (queue rows, in order).
fn three_queued() -> StateSnapshot {
    let tasks = ["t0", "t1", "t2"]
        .iter()
        .map(|id| {
            let mut t = TaskInstance::default();
            t.id = (*id).into();
            t.status = TaskStatus::Queued;
            t.target.repo = "platform".into();
            t
        })
        .collect();
    StateSnapshot {
        tasks,
        projects: vec![Project { name: "platform".into(), github_id: None }],
        ..Default::default()
    }
}

fn three_worktrees() -> StateSnapshot {
    let mut wts = HashMap::new();
    wts.insert("platform".into(), vec![
        WorktreeInfo { name: "wt-a".into(), path: "/wt/a".into(), branch: "wt-a".into(), ..Default::default() },
        WorktreeInfo { name: "wt-b".into(), path: "/wt/b".into(), branch: "wt-b".into(), ..Default::default() },
        WorktreeInfo { name: "wt-c".into(), path: "/wt/c".into(), branch: "wt-c".into(), ..Default::default() },
    ]);
    StateSnapshot {
        projects: vec![Project { name: "platform".into(), github_id: None }],
        worktrees: wts,
        ..Default::default()
    }
}

fn marks(a: &App, pane: ListPane) -> HashSet<String> {
    a.active_ui().marks[pane.idx()].clone()
}

#[test]
fn space_toggles_the_cursor_row_mark_on_and_off() {
    let mut a = app_with(three_queued());
    let u = a.update(space());
    assert!(u.dirty);
    assert_eq!(marks(&a, ListPane::Queue), HashSet::from(["t0".to_string()]));
    // Toggling the same row again removes it (idempotent round-trip).
    a.update(space());
    assert!(marks(&a, ListPane::Queue).is_empty());
}

#[test]
fn space_does_not_move_the_cursor_or_touch_the_anchor() {
    let mut a = app_with(three_queued());
    a.update(down()); // cursor → row 1
    a.update(space());
    let sel = a.active_ui().selections[ListPane::Queue.idx()];
    assert_eq!(sel, Selection { cursor: 1, anchor: None }, "mark is toggle-in-place");
    assert_eq!(marks(&a, ListPane::Queue), HashSet::from(["t1".to_string()]));
}

#[test]
fn marks_accumulate_across_non_adjacent_rows() {
    let mut a = app_with(three_queued());
    a.update(space()); // mark t0
    a.update(down());
    a.update(down()); // cursor → t2, skipping t1
    a.update(space()); // mark t2
    assert_eq!(marks(&a, ListPane::Queue), HashSet::from(["t0".to_string(), "t2".to_string()]));
}

#[test]
fn cursor_movement_preserves_marks() {
    // Moving the cursor clears the ANCHOR (set_cursor does that today) but must
    // NOT clear marks — moving between rows is exactly how you reach the next
    // row you want to mark.
    let mut a = app_with(three_queued());
    a.update(space());
    a.update(down());
    a.update(down());
    assert_eq!(marks(&a, ListPane::Queue), HashSet::from(["t0".to_string()]));
}

#[test]
fn marks_survive_a_snapshot_push_that_reorders_rows() {
    // Identity-keyed, so a daemon push that reshuffles row order can't
    // invalidate a mark the way an index-keyed one would.
    let mut a = app_with(three_queued());
    a.update(down()); // cursor → t1
    a.update(space()); // mark t1
    a.update(Event::Snapshot(three_queued()));
    assert_eq!(marks(&a, ListPane::Queue), HashSet::from(["t1".to_string()]));
}

#[test]
fn marks_survive_a_search_filter_edit() {
    // The contiguous range is wiped on every search keystroke (update.rs resets
    // Selection, since a filtered-index range is meaningless once the filter
    // changes). Identity-keyed marks must survive it.
    let mut a = app_with(three_queued());
    a.update(space()); // mark t0
    a.update(key('/')); // open search on the queue pane
    a.update(key('t')); // type into the filter
    assert_eq!(marks(&a, ListPane::Queue), HashSet::from(["t0".to_string()]));
}

#[test]
fn marks_are_scoped_per_pane() {
    let mut a = app_with(three_worktrees());
    a.update(space()); // mark queue row (queue is empty here → no-op)
    a.update(tab());
    a.update(tab()); // → worktrees
    a.update(space()); // mark wt-a
    assert_eq!(marks(&a, ListPane::Worktrees), HashSet::from(["wt-a".to_string()]));
    assert!(marks(&a, ListPane::Tasks).is_empty());
}

#[test]
fn space_on_an_empty_pane_is_inert() {
    // No snapshot rows → row_identity returns None → nothing to toggle, no panic.
    let snap = StateSnapshot {
        projects: vec![Project { name: "platform".into(), github_id: None }],
        ..Default::default()
    };
    let mut a = app_with(snap);
    let u = a.update(space());
    assert!(!u.dirty);
    assert!(marks(&a, ListPane::Queue).is_empty());
}

/// Render the app to a test buffer and return every line as a String, paired
/// with whether that line carries the selection background (the same accent bg
/// the contiguous range paints).
fn rendered_selected_rows(a: &App) -> Vec<String> {
    use ratatui::{Terminal, backend::TestBackend};
    let mut term = Terminal::new(TestBackend::new(120, 40)).expect("test terminal");
    // NOTE: `render`'s signature is `render(app, frame)` — app first.
    term.draw(|f| { crate::view::render(a, f); }).expect("draw");
    let buf = term.backend().buffer().clone();
    // Selected rows are painted with `Palette::selection()`, whose bg is
    // `selection_bg` (see `view/theme.rs`).
    let sel_bg = crate::view::theme::Palette::default().selection_bg;
    let mut out = Vec::new();
    for y in 0..buf.area.height {
        let mut line = String::new();
        let mut selected = false;
        for x in 0..buf.area.width {
            let cell = &buf[(x, y)];
            if cell.bg == sel_bg {
                selected = true;
            }
            line.push_str(cell.symbol());
        }
        if selected {
            out.push(line.trim().to_string());
        }
    }
    out
}

#[test]
fn esc_clears_range_and_marks_together_in_one_press() {
    // Range and marks are ONE selection from the user's side — a single Esc
    // drops both, rather than making them peel it back in two presses.
    let mut a = app_with(three_queued());
    a.update(Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT))); // range t0..t1
    a.update(space()); // mark t1 (the cursor row)
    assert!(!marks(&a, ListPane::Queue).is_empty());
    a.update(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    assert!(marks(&a, ListPane::Queue).is_empty(), "marks cleared");
    assert_eq!(a.active_ui().selections[ListPane::Queue.idx()].anchor, None, "range cleared");
}

#[test]
fn esc_clears_marks_alone_before_falling_through_to_search() {
    // Marks with no range still occupy Esc's first stage — the search filter is
    // only cleared by a SECOND Esc, matching how a range behaves today.
    // (The rows need a non-empty `summary` — derived from `prompt` — for a
    // non-empty filter to keep any of them visible; `three_queued`'s default
    // `TaskInstance` has an empty prompt, so it's set here to something that
    // matches "t".)
    let mut snap = three_queued();
    for t in snap.tasks.iter_mut() {
        t.prompt = format!("{} task", t.id);
    }
    let mut a = app_with(snap);
    a.update(key('/'));
    a.update(key('t')); // filter = "t" (matches all three summaries)
    a.update(Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))); // apply, back to List
    a.update(space()); // mark the cursor row
    a.update(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    assert!(marks(&a, ListPane::Queue).is_empty(), "first Esc clears marks");
    assert_eq!(a.active_ui().search[ListPane::Queue.idx()], "t", "search survives the first Esc");
    a.update(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    assert!(a.active_ui().search[ListPane::Queue.idx()].is_empty(), "second Esc clears search");
}

#[test]
fn firing_a_bulk_action_clears_the_marks() {
    // A completed bulk action must not leave marks behind to silently widen the
    // NEXT action's selection.
    let mut a = app_with(three_worktrees());
    a.update(tab());
    a.update(tab()); // → worktrees
    a.update(space()); // mark wt-a
    a.update(key('x')); // bulk-remove confirm
    a.update(key('y')); // confirm
    assert!(marks(&a, ListPane::Worktrees).is_empty(), "marks cleared after dispatch");
}

#[test]
fn a_marked_row_renders_with_the_selection_highlight() {
    // Two-tone follow-up: `rendered_selected_rows` only tracks the BRIGHT
    // `selection_bg` bar (see its doc comment). Post-fix, the cursor row (wt-c)
    // carries that bright bar so it never goes invisible once a mark exists,
    // while a marked non-cursor row (wt-b) carries the dimmer muted bar instead
    // — see `cursor_row_is_bright_and_a_marked_non_cursor_row_is_muted` below
    // for the two-channel (bright + muted) version of this same scenario.
    let mut a = app_with(three_worktrees());
    a.update(tab());
    a.update(tab()); // → worktrees (focused; highlight only paints when focused)
    a.update(down()); // cursor → wt-b
    a.update(space()); // mark wt-b
    a.update(down()); // cursor → wt-c, leaving wt-b marked but not under the cursor
    let lines = rendered_selected_rows(&a);
    assert!(
        lines.iter().any(|l| l.contains("wt-c")),
        "the cursor row stays bright-highlighted so it never goes invisible: {lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l.contains("wt-b")),
        "a marked non-cursor row renders the dimmer muted bar, not the bright one: {lines:?}"
    );
}

#[test]
fn cursor_row_is_bright_and_a_marked_non_cursor_row_is_muted() {
    use ratatui::{Terminal, backend::TestBackend};
    let mut a = app_with(three_worktrees());
    a.update(tab());
    a.update(tab()); // → worktrees, focused
    a.update(down()); // cursor → wt-b
    a.update(space()); // mark wt-b
    a.update(down()); // cursor → wt-c (wt-b now marked but not the cursor)

    let mut term = Terminal::new(TestBackend::new(120, 40)).expect("term");
    term.draw(|f| { crate::view::render(&a, f); }).expect("draw"); // NOTE: render(app, frame)
    let buf = term.backend().buffer().clone();
    let bright = crate::view::theme::Palette::default().selection_bg;
    let muted = crate::view::theme::Palette::default().selection_muted_bg;

    // For each rendered row line, record which bar bg (if any) it carries.
    let mut bright_rows = Vec::new();
    let mut muted_rows = Vec::new();
    for y in 0..buf.area.height {
        let mut line = String::new();
        let (mut has_bright, mut has_muted) = (false, false);
        for x in 0..buf.area.width {
            let cell = &buf[(x, y)];
            if cell.bg == bright { has_bright = true; }
            if cell.bg == muted { has_muted = true; }
            line.push_str(cell.symbol());
        }
        if has_bright { bright_rows.push(line.clone()); }
        if has_muted { muted_rows.push(line); }
    }

    assert!(bright_rows.iter().any(|l| l.contains("wt-c")), "cursor row wt-c must be bright: {bright_rows:?}");
    assert!(muted_rows.iter().any(|l| l.contains("wt-b")), "marked non-cursor row wt-b must be muted: {muted_rows:?}");
    // The cursor row must NOT also carry the muted bg, and the marked row must NOT carry the bright bg.
    assert!(!muted_rows.iter().any(|l| l.contains("wt-c")), "cursor row must not be muted");
    assert!(!bright_rows.iter().any(|l| l.contains("wt-b")), "marked non-cursor row must not be bright");
}
