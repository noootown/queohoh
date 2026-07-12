use super::*;
use crate::ipc::types::{Project, StateSnapshot, TaskInstance, TaskStatus, WorktreeInfo};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;

fn key(c: char) -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
}
fn enter() -> Event {
    Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
}
// Menu navigation moved to arrow keys (letters now type into the filter).
fn down() -> Event {
    Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
}
fn up() -> Event {
    Event::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE))
}
fn shift_down() -> Event {
    Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT))
}

fn app_with(snap: StateSnapshot) -> App {
    let mut a = App::new("/tmp/runs".into(), "/tmp/daemon.sock".into());
    a.size = (120, 40);
    a.update(Event::Snapshot(snap));
    a
}

fn failed_task_snapshot() -> StateSnapshot {
    let mut t = TaskInstance::default();
    t.id = "t1".into();
    t.status = TaskStatus::Failed;
    t.target.repo = "platform".into();
    StateSnapshot {
        tasks: vec![t],
        projects: vec![Project { name: "platform".into(), github_id: None }],
        ..Default::default()
    }
}

fn worktree_snapshot() -> StateSnapshot {
    let mut wts = HashMap::new();
    wts.insert(
        "platform".into(),
        vec![WorktreeInfo { name: "platform.wt-a".into(), path: "/wt/wt-a".into(), branch: "wt-a".into(), ..Default::default() }],
    );
    StateSnapshot {
        projects: vec![Project { name: "platform".into(), github_id: None }],
        worktrees: wts,
        ..Default::default()
    }
}

// --- focus helpers (Tab cycles panes; moveFocus order queue→tasks→worktrees) ---
fn tab(a: &mut App) {
    a.update(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
}
fn focus_tasks(a: &mut App) {
    tab(a);
}
fn focus_worktrees(a: &mut App) {
    tab(a);
    tab(a);
}

// --- queue `r` / `x` chip-keys (the queue menu's old verbs are now keys) ------

#[test]
fn queue_r_requeues_the_selected_failed_task() {
    // `r` on QUEUE re-queues the selected task via the retry RPC (single row).
    let mut a = app_with(failed_task_snapshot());
    let u = a.update(key('r'));
    assert!(matches!(a.mode, Mode::List));
    assert!(
        u.cmds.iter().any(|c| matches!(c, Cmd::Rpc { call, .. }
            if call.method == "retry" && call.params == serde_json::json!({ "id": "t1" }))),
        "expected retry Cmd, got {:?}", u.cmds,
    );
}

#[test]
fn queue_r_noop_on_running_sets_status_line() {
    let mut snap = failed_task_snapshot();
    snap.tasks[0].status = TaskStatus::Running;
    let mut a = app_with(snap);
    let u = a.update(key('r'));
    assert!(u.cmds.is_empty(), "running is not re-queueable");
    assert!(a.status_line.as_deref().unwrap_or("").contains("requeue"), "status: {:?}", a.status_line);
}

#[test]
fn queue_x_confirms_then_stops_a_running_task() {
    // `x` always opens the confirm dialog first; Enter fires the stop.
    let mut snap = failed_task_snapshot();
    snap.tasks[0].status = TaskStatus::Running;
    let mut a = app_with(snap);
    let opened = a.update(key('x'));
    assert!(opened.cmds.is_empty(), "x opens the dialog, dispatches nothing yet");
    match &a.mode {
        Mode::ConfirmCancel { calls, .. } => {
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].method, "stop");
        }
        other => panic!("expected ConfirmCancel, got {other:?}"),
    }
    let u = a.update(enter()); // confirm
    assert!(matches!(a.mode, Mode::List));
    match u.cmds.iter().find(|c| matches!(c, Cmd::RpcSeq { .. })).unwrap() {
        Cmd::RpcSeq { verb, calls, .. } => {
            assert_eq!(verb, "cancelled");
            assert_eq!(calls[0].method, "stop");
            assert_eq!(calls[0].params, serde_json::json!({ "id": "t1" }));
        }
        _ => unreachable!(),
    }
}

#[test]
fn queue_x_confirms_then_skips_a_queued_task() {
    let mut snap = failed_task_snapshot();
    snap.tasks[0].status = TaskStatus::Queued;
    let mut a = app_with(snap);
    a.update(key('x'));
    assert!(matches!(a.mode, Mode::ConfirmCancel { .. }));
    let u = a.update(enter());
    assert!(u.cmds.iter().any(|c| matches!(c, Cmd::RpcSeq { calls, .. }
        if calls[0].method == "skip" && calls[0].params == serde_json::json!({ "id": "t1" }))));
}

#[test]
fn queue_x_esc_dismisses_the_confirm_without_dispatch() {
    let mut snap = failed_task_snapshot();
    snap.tasks[0].status = TaskStatus::Running;
    let mut a = app_with(snap);
    a.update(key('x'));
    assert!(matches!(a.mode, Mode::ConfirmCancel { .. }));
    let u = a.update(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    assert!(matches!(a.mode, Mode::List), "esc dismisses");
    assert!(u.cmds.is_empty(), "esc dispatches nothing");
}

#[test]
fn queue_x_noop_on_terminal_sets_status_line_without_dialog() {
    // A done (terminal) task can't be cancelled: status line, no dialog, no cmd.
    let mut snap = failed_task_snapshot();
    snap.tasks[0].status = TaskStatus::Done;
    let mut a = app_with(snap);
    let u = a.update(key('x'));
    assert!(matches!(a.mode, Mode::List), "no dialog when nothing is cancellable");
    assert!(u.cmds.is_empty());
    assert!(a.status_line.as_deref().unwrap_or("").contains("cancel"), "status: {:?}", a.status_line);
}

#[test]
fn queue_needs_input_is_requeueable_and_cancellable_via_skip() {
    // Needs-input: `r` re-queues immediately (retry); `x` confirms then skips.
    let mut snap = failed_task_snapshot();
    snap.tasks[0].status = TaskStatus::NeedsInput;
    let mut a = app_with(snap);
    let ru = a.update(key('r'));
    assert!(ru.cmds.iter().any(|c| matches!(c, Cmd::Rpc { call, .. } if call.method == "retry")));

    let mut snap2 = failed_task_snapshot();
    snap2.tasks[0].status = TaskStatus::NeedsInput;
    let mut b = app_with(snap2);
    b.update(key('x'));
    assert!(matches!(b.mode, Mode::ConfirmCancel { .. }));
    let xu = b.update(enter());
    assert!(xu.cmds.iter().any(|c| matches!(c, Cmd::RpcSeq { calls, .. } if calls[0].method == "skip")));
}

#[test]
fn queue_range_requeue_with_no_eligible_sets_status_line() {
    // A range of only queued/running rows has nothing to re-queue.
    let mut t0 = TaskInstance::default(); t0.id = "t0".into(); t0.status = TaskStatus::Queued; t0.target.repo = "platform".into();
    let mut t1 = TaskInstance::default(); t1.id = "t1".into(); t1.status = TaskStatus::Running; t1.target.repo = "platform".into();
    let snap = StateSnapshot { tasks: vec![t0, t1], projects: vec![Project { name: "platform".into(), github_id: None }], ..Default::default() };
    let mut a = app_with(snap);
    a.update(shift_down());
    let u = a.update(key('r'));
    assert!(u.cmds.is_empty());
    assert!(a.status_line.as_deref().unwrap_or("").contains("re-queueable"), "status: {:?}", a.status_line);
}

#[test]
fn queue_range_requeue_all_eligible_requeues_each() {
    // A range of two failed tasks re-queues both in one RpcSeq.
    let mut t0 = TaskInstance::default(); t0.id = "t0".into(); t0.status = TaskStatus::Failed; t0.target.repo = "platform".into();
    let mut t1 = TaskInstance::default(); t1.id = "t1".into(); t1.status = TaskStatus::Failed; t1.target.repo = "platform".into();
    let snap = StateSnapshot { tasks: vec![t0, t1], projects: vec![Project { name: "platform".into(), github_id: None }], ..Default::default() };
    let mut a = app_with(snap);
    a.update(shift_down());
    let u = a.update(key('r'));
    match u.cmds.iter().find(|c| matches!(c, Cmd::RpcSeq { .. })).unwrap() {
        Cmd::RpcSeq { verb, calls, .. } => {
            assert_eq!(verb, "requeued");
            assert_eq!(calls.len(), 2);
            assert!(calls.iter().all(|c| c.method == "retry"));
        }
        _ => unreachable!(),
    }
}

#[test]
fn queue_range_cancel_all_running_stops_each() {
    // A range of two running tasks stops both in one RpcSeq.
    let mut t0 = TaskInstance::default(); t0.id = "t0".into(); t0.status = TaskStatus::Running; t0.target.repo = "platform".into();
    let mut t1 = TaskInstance::default(); t1.id = "t1".into(); t1.status = TaskStatus::Running; t1.target.repo = "platform".into();
    let snap = StateSnapshot { tasks: vec![t0, t1], projects: vec![Project { name: "platform".into(), github_id: None }], ..Default::default() };
    let mut a = app_with(snap);
    a.update(shift_down());
    a.update(key('x')); // opens confirm
    match &a.mode {
        Mode::ConfirmCancel { summary, .. } => assert!(summary.contains("2 tasks") && summary.contains("stopped")),
        other => panic!("{other:?}"),
    }
    let u = a.update(enter());
    match u.cmds.iter().find(|c| matches!(c, Cmd::RpcSeq { .. })).unwrap() {
        Cmd::RpcSeq { verb, calls, .. } => {
            assert_eq!(verb, "cancelled");
            assert_eq!(calls.len(), 2);
            assert!(calls.iter().all(|c| c.method == "stop"));
        }
        _ => unreachable!(),
    }
    assert_eq!(a.active_ui().selections[0].anchor, None); // range cleared on confirm
}

#[test]
fn queue_range_cancel_with_no_eligible_sets_status_line() {
    // A range of only terminal rows has nothing to cancel.
    let mut t0 = TaskInstance::default(); t0.id = "t0".into(); t0.status = TaskStatus::Done; t0.target.repo = "platform".into();
    let mut t1 = TaskInstance::default(); t1.id = "t1".into(); t1.status = TaskStatus::Failed; t1.target.repo = "platform".into();
    let snap = StateSnapshot { tasks: vec![t0, t1], projects: vec![Project { name: "platform".into(), github_id: None }], ..Default::default() };
    let mut a = app_with(snap);
    a.update(shift_down());
    let u = a.update(key('x'));
    assert!(matches!(a.mode, Mode::List), "no dialog when nothing is cancellable");
    assert!(u.cmds.is_empty());
    assert!(a.status_line.as_deref().unwrap_or("").contains("cancel"), "status: {:?}", a.status_line);
}

#[test]
fn queue_action_menu_is_single_resume_and_disabled_enter_is_inert() {
    // `a` on QUEUE opens the single Resume menu. The fixture task never ran (no
    // session id) and has no worktree, so Resume is disabled regardless of TMUX
    // → Enter is inert and the menu stays open (generic disabled-row guard).
    let mut a = app_with(failed_task_snapshot());
    a.update(key('a'));
    match &a.mode {
        Mode::ActionMenu { items, .. } => {
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].label, "Resume");
            assert!(items[0].disabled.is_some(), "no session/worktree → disabled");
        }
        other => panic!("expected single-item Resume menu, got {other:?}"),
    }
    let u = a.update(enter());
    assert!(matches!(a.mode, Mode::ActionMenu { .. }), "disabled row keeps the menu open");
    assert!(u.cmds.is_empty());
}

// --- generic action-menu navigation (exercised via the WORKTREE menu, which
// still has multiple items now the queue menu is a single row) ----------------

#[test]
fn menu_arrows_move_highlight() {
    let mut a = app_with(worktree_snapshot());
    focus_worktrees(&mut a);
    a.update(key('a')); // worktree menu: 4 items
    a.update(down());
    match &a.mode {
        Mode::ActionMenu { index, .. } => assert_eq!(*index, 1),
        other => panic!("{other:?}"),
    }
    a.update(up());
    match &a.mode {
        Mode::ActionMenu { index, .. } => assert_eq!(*index, 0),
        other => panic!("{other:?}"),
    }
}

#[test]
fn menu_esc_closes_but_q_types_into_filter() {
    let mut a = app_with(worktree_snapshot());
    focus_worktrees(&mut a);
    a.update(key('a'));
    a.update(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    assert!(matches!(a.mode, Mode::List));
    // Reopen: `q` no longer closes — it types into the label filter.
    a.update(key('a'));
    a.update(key('q'));
    match &a.mode {
        Mode::ActionMenu { query, index, .. } => {
            assert_eq!(query, "q");
            assert_eq!(*index, 0);
        }
        other => panic!("expected the menu to stay open, got {other:?}"),
    }
}

#[test]
fn menu_typing_filters_then_enter_executes_through_filter() {
    // Filter the worktree menu to "fresh", then Enter opens the fresh-session
    // AddTask flow — proving Enter resolves the FILTERED highlight.
    let mut a = app_with(worktree_snapshot());
    focus_worktrees(&mut a);
    a.update(key('a'));
    a.update(key('f'));
    a.update(key('r')); // "fr" → only "New task (fresh session)"
    match &a.mode {
        Mode::ActionMenu { items, index, query, .. } => {
            assert_eq!(query, "fr");
            assert_eq!(*index, 0);
            assert_eq!(items.len(), 4); // filter is a view; items unchanged
        }
        other => panic!("{other:?}"),
    }
    a.update(enter());
    assert!(matches!(a.mode, Mode::AddTask { session: SessionMode::Fresh, .. }));
}

#[test]
fn menu_backspace_edits_filter() {
    let mut a = app_with(worktree_snapshot());
    focus_worktrees(&mut a);
    a.update(key('a'));
    a.update(key('f'));
    a.update(key('z')); // "fz" → no matches
    a.update(Event::Key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)));
    match &a.mode {
        Mode::ActionMenu { query, .. } => assert_eq!(query, "f"),
        other => panic!("{other:?}"),
    }
}

fn menu_preview_scroll(a: &App) -> usize {
    match &a.mode {
        Mode::ActionMenu { preview_scroll, .. } => *preview_scroll,
        other => panic!("expected ActionMenu, got {other:?}"),
    }
}

// ctrl+d/u preview paging was removed (user request — wheel-only scroll);
// `menu_wheel_scrolls_preview_and_moves_selection` covers the scroll path.

/// Sets the open ActionMenu's preview scroll directly (the wheel path is
/// exercised by its own test; here we only need a non-zero scroll to reset).
fn set_menu_scroll(a: &mut App, v: usize) {
    if let Mode::ActionMenu { preview_scroll, .. } = &mut a.mode {
        *preview_scroll = v;
    } else {
        panic!("expected ActionMenu");
    }
}

#[test]
fn menu_nav_and_query_edits_reset_preview_scroll() {
    let mut a = app_with(failed_task_snapshot());
    a.update(key('a'));
    set_menu_scroll(&mut a, 3);
    a.update(down()); // highlight moved → preview belongs to a new row
    assert_eq!(menu_preview_scroll(&a), 0);
    set_menu_scroll(&mut a, 3);
    a.update(key('s')); // query edit → filtered view changed
    assert_eq!(menu_preview_scroll(&a), 0);
    set_menu_scroll(&mut a, 3);
    a.update(Event::Key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)));
    assert_eq!(menu_preview_scroll(&a), 0);
}

#[test]
fn menu_wheel_scrolls_preview_and_moves_selection() {
    use crate::hit::HitTarget;
    use crossterm::event::{MouseEvent, MouseEventKind};
    fn wheel(a: &mut App, kind: MouseEventKind, col: u16, row: u16) {
        a.update(Event::Mouse(MouseEvent { kind, column: col, row, modifiers: KeyModifiers::NONE }));
    }
    // The worktree menu (4 items) so the left-panel wheel can move the selection;
    // the queue menu is a single Resume row.
    let mut a = app_with(worktree_snapshot());
    focus_worktrees(&mut a);
    a.update(key('a'));
    // Synthetic picker hit map: left panel Modal + right preview region.
    let mut hits = crate::hit::HitMap::new();
    hits.push(ratatui::layout::Rect { x: 5, y: 5, width: 20, height: 10 }, HitTarget::Modal);
    hits.push(
        ratatui::layout::Rect { x: 50, y: 5, width: 20, height: 10 },
        HitTarget::MenuPreview,
    );
    a.hit = hits;
    a.menu_preview_max_scroll.set(10);
    // The preview scrolls at the shared DETAIL wheel step (WHEEL_STEP = 3 lines
    // per tick), not one line — matching the detail pane so they never drift.
    wheel(&mut a, MouseEventKind::ScrollDown, 55, 6); // over the preview
    assert_eq!(menu_preview_scroll(&a), 3);
    wheel(&mut a, MouseEventKind::ScrollUp, 55, 6);
    assert_eq!(menu_preview_scroll(&a), 0);
    // Over the left panel: the wheel moves the selection (clamped) and the
    // menu stays open.
    wheel(&mut a, MouseEventKind::ScrollDown, 6, 6);
    match &a.mode {
        Mode::ActionMenu { index, preview_scroll, .. } => {
            assert_eq!(*index, 1);
            assert_eq!(*preview_scroll, 0);
        }
        other => panic!("{other:?}"),
    }
    wheel(&mut a, MouseEventKind::ScrollUp, 6, 6);
    match &a.mode {
        Mode::ActionMenu { index, .. } => assert_eq!(*index, 0),
        other => panic!("{other:?}"),
    }
}

#[test]
fn worktree_menu_task_fresh_opens_add_task_with_raw_name() {
    let mut a = app_with(worktree_snapshot());
    focus_worktrees(&mut a);
    a.update(key('a')); // open worktree menu, index 0 = New task (fresh)…
    a.update(enter());
    match &a.mode {
        Mode::AddTask { worktree, session, .. } => {
            assert_eq!(worktree.as_deref(), Some("platform.wt-a"));
            assert!(matches!(session, SessionMode::Fresh));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn worktree_menu_remove_opens_confirm_remove() {
    let mut a = app_with(worktree_snapshot());
    focus_worktrees(&mut a);
    a.update(key('a'));
    for _ in 0..3 {
        a.update(down());
    } // -> Remove worktree (index 3 in the trimmed menu)
    a.update(enter());
    match &a.mode {
        Mode::ConfirmRemove { repo, worktree, branch } => {
            assert_eq!(repo, "platform");
            assert_eq!(worktree, "platform.wt-a"); // raw name for removal
            assert_eq!(branch, "wt-a");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn confirm_remove_y_dispatches_and_n_cancels() {
    let mut a = app_with(worktree_snapshot());
    focus_worktrees(&mut a);
    a.update(key('a'));
    for _ in 0..3 {
        a.update(down());
    }
    a.update(enter()); // ConfirmRemove
    let u = a.update(key('y'));
    assert!(matches!(a.mode, Mode::List));
    assert!(u.cmds.iter().any(|c| matches!(c, Cmd::Rpc { call, .. }
        if call.method == "removeWorktree"
            && call.params == serde_json::json!({ "repo": "platform", "name": "platform.wt-a" }))));

    // n cancels without a cmd
    a.update(key('a'));
    for _ in 0..3 {
        a.update(down());
    }
    a.update(enter());
    let u2 = a.update(key('n'));
    assert!(matches!(a.mode, Mode::List));
    assert!(u2.cmds.is_empty());
}

#[test]
fn tasks_pane_run_zero_arg_def_dispatches_and_closes() {
    // tasks-pane Enter runs the highlighted def directly (no menu hop). A
    // zero-arg def dispatches runDefinition immediately.
    let snap = StateSnapshot {
        projects: vec![Project { name: "platform".into(), github_id: None }],
        ..Default::default()
    };
    let mut a = app_with(snap);
    a.defs_by_project.insert("platform".into(), vec![{
        let mut d = crate::ipc::types::DefinitionSummary::default();
        d.repo = "platform".into();
        d.name = "lint".into();
        d
    }]);
    focus_tasks(&mut a);
    let u = a.update(key('r')); // single `r` → immediate dispatch (zero-arg)
    assert!(matches!(a.mode, Mode::List));
    assert!(
        u.cmds.iter().any(|c| matches!(c, Cmd::Rpc { call, invalidate_defs_for, .. }
            if call.method == "runDefinition"
                && call.params["name"] == "lint"
                && call.params["source"] == "tui"
                && invalidate_defs_for.as_deref() == Some("platform"))),
        "expected an immediate runDefinition dispatch, got {:?}",
        u.cmds,
    );
}

#[test]
fn click_menu_item_executes() {
    use crate::hit::HitTarget;
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    use ratatui::{Terminal, backend::TestBackend};

    // Use the WORKTREE menu (multiple enabled items) so a row click executes a
    // real action — the queue menu is a single (often disabled) Resume row.
    let mut a = app_with(worktree_snapshot());
    focus_worktrees(&mut a);
    a.update(key('a')); // open menu; row 0 = "New task (fresh session)" (enabled)
    assert!(matches!(a.mode, Mode::ActionMenu { .. }));

    // Render the open menu so the real hit map carries its MenuItem regions.
    let (w, h) = (120u16, 40u16);
    a.size = (w, h);
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    let mut hits = crate::hit::HitMap::new();
    term.draw(|f| hits = crate::view::render(&a, f)).unwrap();
    a.hit = hits;

    // Locate a MenuItem(0) cell and synthesize a left-click on it.
    let mut pos = None;
    'find: for y in 0..h {
        for x in 0..w {
            if let Some(HitTarget::MenuItem(0)) = a.hit.hit(x, y) {
                pos = Some((x, y));
                break 'find;
            }
        }
    }
    let (mx, my) = pos.expect("MenuItem(0) region present after render");
    a.update(Event::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: mx,
        row: my,
        modifiers: KeyModifiers::NONE,
    }));
    // Clicking the enabled row 0 executes it — the fresh-session AddTask flow.
    assert!(
        matches!(a.mode, Mode::AddTask { session: SessionMode::Fresh, .. }),
        "clicking 'New task (fresh session)' opens the fresh AddTask, got {:?}", a.mode,
    );
}

#[test]
fn click_outside_menu_closes_it() {
    use crate::hit::HitTarget;
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    use ratatui::{Terminal, backend::TestBackend};

    let mut a = app_with(failed_task_snapshot());
    a.update(key('a'));
    let (w, h) = (120u16, 40u16);
    a.size = (w, h);
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    let mut hits = crate::hit::HitMap::new();
    term.draw(|f| hits = crate::view::render(&a, f)).unwrap();
    a.hit = hits;

    // A corner cell is outside the centered popup (neither Modal nor MenuItem).
    assert!(!matches!(a.hit.hit(0, h - 1), Some(HitTarget::Modal | HitTarget::MenuItem(_))));
    let u = a.update(Event::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 0,
        row: h - 1,
        modifiers: KeyModifiers::NONE,
    }));
    assert!(matches!(a.mode, Mode::List));
    assert!(u.cmds.is_empty());
}
