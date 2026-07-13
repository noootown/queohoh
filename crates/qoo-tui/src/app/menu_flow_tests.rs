use super::*;
use crate::event::SessionChoice;
use crate::ipc::types::{Project, SessionEntry, StateSnapshot, TaskInstance, TaskStatus, WorktreeInfo};
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
fn esc() -> Event {
    Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
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

/// Open a synthetic multi-row `ActionMenu` directly. The single-target worktree
/// menu was retired in Task 9 (its verbs became the `r`/`g`/`x` hotkeys), but
/// the lazyvim-style picker machinery (navigation, filtering, wheel, click) is
/// still live for the queue Resume + bulk menus — these generic-nav tests need a
/// multi-row vehicle. Rows are inert placeholders; execution routes to a
/// harmless `BulkRunDefs` so a click has an observable effect (returns to List).
fn open_synthetic_menu(a: &mut App, n: usize) {
    use crate::action_menu::{ActionItem, MenuAction};
    let items = (0..n)
        .map(|i| ActionItem {
            label: format!("item {i}"),
            disabled: None,
            description: String::new(),
            action: MenuAction::BulkRunDefs { repo: "platform".into(), names: vec![] },
        })
        .collect();
    a.mode = Mode::ActionMenu {
        title: "menu".into(),
        items,
        index: 0,
        query: String::new(),
        preview_scroll: 0,
    };
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
        Mode::Confirm { action: ConfirmAction::CancelTasks { calls }, .. } => {
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].method, "stop");
        }
        other => panic!("expected cancel confirm, got {other:?}"),
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
    assert!(matches!(a.mode, Mode::Confirm { action: ConfirmAction::CancelTasks { .. }, .. }));
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
    assert!(matches!(a.mode, Mode::Confirm { action: ConfirmAction::CancelTasks { .. }, .. }));
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
    assert!(matches!(b.mode, Mode::Confirm { action: ConfirmAction::CancelTasks { .. }, .. }));
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
        Mode::Confirm { body, action: ConfirmAction::CancelTasks { .. }, .. } => {
            assert!(body[0].contains("2 tasks") && body[0].contains("stopped"))
        }
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
    open_synthetic_menu(&mut a, 4); // multi-row picker vehicle
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
    open_synthetic_menu(&mut a, 2);
    a.update(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    assert!(matches!(a.mode, Mode::List));
    // Reopen: `q` no longer closes — it types into the label filter.
    open_synthetic_menu(&mut a, 2);
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
fn menu_backspace_edits_filter() {
    let mut a = app_with(worktree_snapshot());
    open_synthetic_menu(&mut a, 2);
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
    // A 4-row menu so the left-panel wheel can move the selection (the queue
    // menu is a single Resume row).
    let mut a = app_with(worktree_snapshot());
    open_synthetic_menu(&mut a, 4);
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

// --- worktrees `r`/`g`/`x` hotkeys (replace the retired worktree menu) --------

// --- session picker (`r` on a worktree row) -----------------------------------

/// Two newest-first loaded sessions for `worktree`, mirroring the `listSessions`
/// reply shape.
fn loaded(worktree: &str) -> Event {
    Event::SessionsLoaded {
        worktree: worktree.into(),
        result: Ok(vec![
            SessionChoice { session_id: "sess-1".into(), label: "Fix the parser".into(), mtime_ms: 2_000 },
            SessionChoice { session_id: "sess-2".into(), label: "Redesign TUI".into(), mtime_ms: 1_000 },
        ]),
    }
}

#[test]
fn r_on_worktree_opens_session_pick_and_fetches() {
    let mut a = app_with(worktree_snapshot());
    focus_worktrees(&mut a);
    let up = a.update(key('r'));
    assert!(matches!(&a.mode, Mode::SessionPick { worktree, loading: true, items, .. }
        if worktree == "platform.wt-a" && items.is_empty()));
    assert!(matches!(&up.cmds[..], [Cmd::FetchSessions { repo, worktree }]
        if repo == "platform" && worktree == "platform.wt-a"));
}

#[test]
fn sessions_loaded_fills_items_and_enter_on_first_row_is_new_session() {
    let mut a = app_with(worktree_snapshot());
    focus_worktrees(&mut a);
    a.update(key('r'));
    a.update(loaded("platform.wt-a"));
    // Loaded items fill and loading clears.
    assert!(matches!(&a.mode, Mode::SessionPick { items, loading: false, .. } if items.len() == 2));
    // Row 0 is the synthetic "New session"; loaded sessions follow. Enter opens
    // the launch form (model + prompt) targeting the worktree, no session pinned.
    a.update(enter());
    assert!(matches!(&a.mode,
        Mode::Form { action: FormAction::NewSession { resume_session_id: None, worktree: w, .. }, .. }
        if w == "platform.wt-a"));
}

#[test]
fn picking_a_session_pins_it() {
    let mut a = app_with(worktree_snapshot());
    focus_worktrees(&mut a);
    a.update(key('r'));
    a.update(loaded("platform.wt-a"));
    // Rows: New(0), Create Worktree(1), sessions(2..) — two downs reach sess-1.
    a.update(down());
    a.update(down());
    a.update(enter());
    // The launch form pins the session; the label rides in the form title.
    match &a.mode {
        Mode::Form { state, action: FormAction::NewSession { resume_session_id: Some(s), .. } } => {
            assert_eq!(s, "sess-1");
            assert!(state.title.contains("Fix the parser"), "title: {}", state.title);
        }
        other => panic!("expected NewSession resume form, got {other:?}"),
    }
}

#[test]
fn launcher_tab_focuses_cancel_and_enter_closes() {
    let mut a = app_with(worktree_snapshot());
    focus_worktrees(&mut a);
    a.update(key('r'));
    a.update(loaded("platform.wt-a"));
    // Tab moves focus onto Cancel; Enter then closes the launcher.
    tab(&mut a);
    assert!(matches!(&a.mode, Mode::SessionPick { focus: crate::hit::ButtonKind::Cancel, .. }));
    a.update(enter());
    assert!(matches!(&a.mode, Mode::List));
}

#[test]
fn launcher_create_worktree_row_opens_the_form() {
    let mut a = app_with(worktree_snapshot());
    focus_worktrees(&mut a);
    a.update(key('r'));
    a.update(loaded("platform.wt-a"));
    // Row 1 = Create Worktree; Enter opens the create-worktree launch form
    // (branch/name + model + prompt) for the active repo.
    a.update(down());
    a.update(enter());
    assert!(matches!(&a.mode,
        Mode::Form { state, action: FormAction::CreateWorktree { repo } }
        if repo == "platform" && state.fields.len() == 3));
}

#[test]
fn session_pick_type_to_filter_matches_labels() {
    let mut a = app_with(worktree_snapshot());
    focus_worktrees(&mut a);
    a.update(key('r'));
    a.update(loaded("platform.wt-a"));
    for c in "redesign".chars() {
        a.update(key(c));
    }
    a.update(enter());
    assert!(matches!(&a.mode,
        Mode::Form { action: FormAction::NewSession { resume_session_id: Some(s), .. }, .. }
        if s == "sess-2"));
}

#[test]
fn stale_sessions_loaded_for_other_worktree_is_ignored_and_esc_cancels() {
    let mut a = app_with(worktree_snapshot());
    focus_worktrees(&mut a);
    a.update(key('r'));
    a.update(loaded("platform.other"));
    assert!(matches!(&a.mode, Mode::SessionPick { loading: true, .. }));
    a.update(esc());
    assert!(matches!(a.mode, Mode::List));
}

#[test]
fn session_pick_load_error_keeps_modal_and_sets_status() {
    // An Err reply clears loading, keeps the picker open (New session still
    // selectable), and surfaces the error on the status line.
    let mut a = app_with(worktree_snapshot());
    focus_worktrees(&mut a);
    a.update(key('r'));
    a.update(Event::SessionsLoaded {
        worktree: "platform.wt-a".into(),
        result: Err("boom".into()),
    });
    assert!(matches!(&a.mode, Mode::SessionPick { loading: false, items, .. } if items.is_empty()));
    assert!(a.status_line.as_deref().unwrap_or("").contains("boom"), "status: {:?}", a.status_line);
    // "New session" (row 0) is still selectable → Enter opens the launch form.
    a.update(enter());
    assert!(matches!(&a.mode,
        Mode::Form { action: FormAction::NewSession { resume_session_id: None, worktree: w, .. }, .. }
        if w == "platform.wt-a"));
}

#[test]
fn x_on_worktree_row_opens_confirm_remove_and_y_dispatches_rpc() {
    let mut a = app_with(worktree_snapshot());
    focus_worktrees(&mut a);
    a.update(key('x'));
    assert!(matches!(&a.mode, Mode::Confirm { action: ConfirmAction::RemoveWorktree { .. }, .. }));
    let up = a.update(key('y'));
    assert!(matches!(&up.cmds[..], [Cmd::Rpc { call, .. }]
        if call.method == "removeWorktree"
        && call.params == serde_json::json!({"repo": "platform", "name": "platform.wt-a"})));
}

#[test]
fn g_on_worktree_row_opens_tmux_when_inside_tmux() {
    let mut a = app_with(worktree_snapshot());
    a.inside_tmux = true;
    focus_worktrees(&mut a);
    let up = a.update(key('g'));
    assert!(matches!(&up.cmds[..], [Cmd::OpenTmux { path }] if path == "/wt/wt-a"));
}

#[test]
fn g_on_worktree_row_noop_outside_tmux_sets_status() {
    let mut a = app_with(worktree_snapshot());
    a.inside_tmux = false;
    focus_worktrees(&mut a);
    let up = a.update(key('g'));
    assert!(up.cmds.is_empty(), "no tmux → no OpenTmux");
    assert!(a.status_line.as_deref().unwrap_or("").contains("tmux"), "status: {:?}", a.status_line);
}

#[test]
fn r_and_x_are_noops_on_session_rows_but_g_works() {
    // A snapshot with one real worktree and one interactive session whose cwd is
    // inside it. The session row is appended after the worktree row, so moving
    // the cursor down once selects it.
    let mut wts = HashMap::new();
    wts.insert(
        "platform".into(),
        vec![WorktreeInfo { name: "platform.wt-a".into(), path: "/wt/wt-a".into(), branch: "wt-a".into(), ..Default::default() }],
    );
    let snap = StateSnapshot {
        projects: vec![Project { name: "platform".into(), github_id: None }],
        worktrees: wts,
        sessions: vec![SessionEntry {
            kind: "interactive".into(),
            key: "s1".into(),
            lane: None,
            cwd: Some("/wt/wt-a/nested".into()),
            pid: None,
            started_at: String::new(),
            heartbeat_at: String::new(),
        }],
        ..Default::default()
    };
    let mut a = app_with(snap);
    a.inside_tmux = true;
    focus_worktrees(&mut a);
    a.update(down()); // select the appended session row (index 1)

    // `r`: no mode change, a status line explaining sessions can't host a task.
    let ru = a.update(key('r'));
    assert!(matches!(a.mode, Mode::List), "r must not open AddTask on a session row");
    assert!(ru.cmds.is_empty());
    assert!(a.status_line.as_deref().unwrap_or("").contains("session"), "status: {:?}", a.status_line);

    // `x`: no mode change, a status line (a session is not a worktree).
    a.status_line = None;
    let xu = a.update(key('x'));
    assert!(matches!(a.mode, Mode::List), "x must not confirm-remove a session row");
    assert!(xu.cmds.is_empty());
    assert!(a.status_line.is_some(), "x sets a status line on a session row");

    // `g`: opens the session's cwd in tmux (works for session rows too).
    let gu = a.update(key('g'));
    assert!(matches!(&gu.cmds[..], [Cmd::OpenTmux { path }] if path == "/wt/wt-a/nested"));
}

#[test]
fn a_no_longer_opens_a_menu_on_worktrees() {
    let mut a = app_with(worktree_snapshot());
    focus_worktrees(&mut a);
    a.update(key('a'));
    assert!(matches!(a.mode, Mode::List));
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

    // A synthetic multi-row menu (all rows enabled) so a row click executes a
    // real action — the queue menu is a single (often disabled) Resume row.
    let mut a = app_with(worktree_snapshot());
    open_synthetic_menu(&mut a, 4); // row 0 = enabled BulkRunDefs
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
    let u = a.update(Event::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: mx,
        row: my,
        modifiers: KeyModifiers::NONE,
    }));
    // Clicking the enabled row 0 executes it — the menu closes (→ List) and the
    // action fires (the BulkRunDefs vehicle dispatches an RpcSeq).
    assert!(matches!(a.mode, Mode::List), "clicking an enabled row closes the menu, got {:?}", a.mode);
    assert!(
        u.cmds.iter().any(|c| matches!(c, Cmd::RpcSeq { .. })),
        "clicking the enabled row executes its action, got {:?}", u.cmds,
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
