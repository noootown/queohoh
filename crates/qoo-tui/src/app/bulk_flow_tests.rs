use super::*;
use crate::hit::HitTarget;
use ratatui::layout::Rect;
use crate::ipc::types::{Project, StateSnapshot, TaskInstance, TaskStatus, WorktreeInfo};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use std::collections::HashMap;

fn key(c: char) -> Event { Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)) }
fn enter() -> Event { Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)) }
fn down() -> Event { Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)) }
fn shift_down() -> Event { Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT)) }
fn mouse(kind: MouseEventKind, col: u16, row: u16) -> Event {
    Event::Mouse(MouseEvent { kind, column: col, row, modifiers: KeyModifiers::NONE })
}

fn app_with(snap: StateSnapshot) -> App {
    let mut a = App::new("/tmp/runs".into(), "/tmp/s.sock".into());
    a.size = (120, 40);
    a.update(Event::Snapshot(snap));
    a
}

fn two_queue_one_failed() -> StateSnapshot {
    let mut t0 = TaskInstance::default(); t0.id = "t0".into(); t0.status = TaskStatus::Failed; t0.target.repo = "platform".into();
    let mut t1 = TaskInstance::default(); t1.id = "t1".into(); t1.status = TaskStatus::Queued; t1.target.repo = "platform".into();
    StateSnapshot { tasks: vec![t0, t1], projects: vec![Project { name: "platform".into(), github_id: None }], ..Default::default() }
}

#[test]
fn queue_range_requeue_via_r_hits_only_eligible_and_clears_range() {
    // t0 failed (eligible), t1 queued (ineligible). `r` over the 2-row range
    // re-queues only t0, with the "requeued" count feedback, and clears the range.
    let mut a = app_with(two_queue_one_failed());
    a.update(shift_down()); // extend queue selection to 2 rows
    let u = a.update(key('r'));
    assert!(matches!(a.mode, Mode::List));
    match u.cmds.iter().find(|c| matches!(c, Cmd::RpcSeq { .. })).unwrap() {
        Cmd::RpcSeq { verb, calls, invalidate_defs_for } => {
            assert_eq!(verb, "requeued");
            assert_eq!(calls.len(), 1); // only t0 is failed
            assert_eq!(calls[0].method, "retry");
            assert_eq!(calls[0].params, serde_json::json!({ "id": "t0" }));
            assert_eq!(*invalidate_defs_for, None);
        }
        _ => unreachable!(),
    }
    assert_eq!(a.active_ui().selections[0].anchor, None); // range cleared
}

#[test]
fn queue_range_cancel_via_x_mixes_stop_and_skip_per_row() {
    // t0 running → stop, t1 queued → skip: one RpcSeq (verb "cancelled") with a
    // per-row method, in row order. Range cleared before dispatch.
    let mut t0 = TaskInstance::default(); t0.id = "t0".into(); t0.status = TaskStatus::Running; t0.target.repo = "platform".into();
    let mut t1 = TaskInstance::default(); t1.id = "t1".into(); t1.status = TaskStatus::Queued; t1.target.repo = "platform".into();
    let snap = StateSnapshot { tasks: vec![t0, t1], projects: vec![Project { name: "platform".into(), github_id: None }], ..Default::default() };
    let mut a = app_with(snap);
    a.update(shift_down());
    a.update(key('x')); // opens the confirm dialog (freezing the calls)
    assert!(matches!(a.mode, Mode::Confirm { action: ConfirmAction::CancelTasks { .. }, .. }));
    let u = a.update(enter()); // confirm
    match u.cmds.iter().find(|c| matches!(c, Cmd::RpcSeq { .. })).unwrap() {
        Cmd::RpcSeq { verb, calls, invalidate_defs_for } => {
            assert_eq!(verb, "cancelled");
            assert_eq!(calls.len(), 2);
            assert_eq!(calls[0].method, "stop"); // t0 running
            assert_eq!(calls[0].params, serde_json::json!({ "id": "t0" }));
            assert_eq!(calls[1].method, "skip"); // t1 queued
            assert_eq!(calls[1].params, serde_json::json!({ "id": "t1" }));
            assert_eq!(*invalidate_defs_for, None);
        }
        _ => unreachable!(),
    }
    assert_eq!(a.active_ui().selections[0].anchor, None); // range cleared on confirm
}

#[test]
fn tasks_bulk_range_via_r_refuses_not_applicable() {
    // TASKS keeps no bulk-doable verb (see `crate::hit::bulk_allowed`): a
    // multi-row range on `r` refuses with a status line instead of the old
    // bulk-run menu, and never enters `Mode::ActionMenu`.
    let mut snap = StateSnapshot { projects: vec![Project { name: "platform".into(), github_id: None }], ..Default::default() };
    snap.tasks = vec![];
    let mut a = app_with(snap);
    a.defs_by_project.insert("platform".into(), vec![
        { let mut d = crate::ipc::types::DefinitionSummary::default(); d.repo = "platform".into(); d.name = "lint".into(); d },
        { let mut d = crate::ipc::types::DefinitionSummary::default(); d.repo = "platform".into(); d.name = "deploy".into(); d.args = vec![crate::ipc::types::ArgSpec { name: "env".into(), ..Default::default() }]; d },
    ]);
    // Focus the tasks pane, extend the selection to 2 rows, then press `r`.
    a.update(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
    a.update(shift_down());
    let u = a.update(key('r'));
    assert!(matches!(a.mode, Mode::List));
    assert_eq!(a.status_line.as_deref(), Some("not applicable to bulk selection"));
    assert!(!u.cmds.iter().any(|c| matches!(c, Cmd::RpcSeq { .. } | Cmd::Rpc { .. })));
}

fn three_worktrees() -> StateSnapshot {
    let mut wts = HashMap::new();
    wts.insert("platform".into(), vec![
        WorktreeInfo { name: "wt-a".into(), path: "/wt/a".into(), branch: "wt-a".into(), ..Default::default() },
        WorktreeInfo { name: "wt-b".into(), path: "/wt/b".into(), branch: "wt-b".into(), ..Default::default() },
        WorktreeInfo { name: "wt-c".into(), path: "/wt/c".into(), branch: "wt-c".into(), ..Default::default() },
    ]);
    StateSnapshot { projects: vec![Project { name: "platform".into(), github_id: None }], worktrees: wts, ..Default::default() }
}

#[test]
fn bulk_remove_confirms_then_rpcseq_removes_each() {
    let mut a = app_with(three_worktrees());
    a.update(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))); // → tasks
    a.update(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))); // → worktrees
    a.update(shift_down()); a.update(shift_down()); // 3-row range
    a.update(key('x')); // worktrees `x` on a range opens the bulk remove menu
    a.update(enter()); // Remove worktrees… → confirm dialog (bulk remove)
    match &a.mode { Mode::Confirm { action: ConfirmAction::BulkRemoveWorktrees { names, .. }, .. } => assert_eq!(names.len(), 3), other => panic!("{other:?}") }
    let u = a.update(key('y'));
    assert!(matches!(a.mode, Mode::List));
    match u.cmds.iter().find(|c| matches!(c, Cmd::RpcSeq { .. })).unwrap() {
        Cmd::RpcSeq { verb, calls, .. } => {
            assert_eq!(verb, "removed");
            assert_eq!(calls.len(), 3);
            assert_eq!(calls[0].params, serde_json::json!({ "repo": "platform", "name": "wt-a" }));
            assert_eq!(calls[2].params, serde_json::json!({ "repo": "platform", "name": "wt-c" }));
        }
        _ => unreachable!(),
    }
    assert_eq!(a.active_ui().selections[2].anchor, None); // range cleared
}

#[test]
fn bulk_remove_n_cancels_without_cmd() {
    let mut a = app_with(three_worktrees());
    a.update(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
    a.update(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
    a.update(shift_down());
    a.update(key('x')); // worktrees `x` on a range opens the bulk remove menu
    a.update(enter());
    let u = a.update(key('n'));
    assert!(matches!(a.mode, Mode::List));
    assert!(u.cmds.is_empty());
}

#[test]
fn worktrees_bulk_range_refuses_run_goto_and_tasks_menu() {
    // WORKTREES keeps only `Remove` bulk-doable — `r`/`g`/`t` all refuse with a
    // status line on a multi-row range instead of silently targeting the
    // cursor row's single worktree.
    let mut a = app_with(three_worktrees());
    a.update(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
    a.update(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
    a.update(shift_down());

    a.update(key('r'));
    assert!(matches!(a.mode, Mode::List));
    assert_eq!(a.status_line.as_deref(), Some("not applicable to bulk selection"));

    a.status_line = None;
    let u = a.update(key('g'));
    assert!(matches!(a.mode, Mode::List));
    assert_eq!(a.status_line.as_deref(), Some("not applicable to bulk selection"));
    assert!(!u.cmds.iter().any(|c| matches!(c, Cmd::OpenTmux { .. })));

    a.status_line = None;
    a.update(key('t'));
    assert!(matches!(a.mode, Mode::List));
    assert_eq!(a.status_line.as_deref(), Some("not applicable to bulk selection"));
}

#[test]
fn queue_bulk_range_refuses_actions_create_and_collapse() {
    // QUEUE keeps only `Run`/`Cancel` (rerun/stop) bulk-doable — `a`/`c`/`z`
    // all refuse with a status line on a multi-row range.
    let mut a = app_with(two_queue_one_failed());
    a.update(shift_down());

    a.update(key('a'));
    assert!(matches!(a.mode, Mode::List));
    assert_eq!(a.status_line.as_deref(), Some("not applicable to bulk selection"));

    a.status_line = None;
    a.update(key('c'));
    assert!(matches!(a.mode, Mode::List)); // no AddTask prompt opened
    assert_eq!(a.status_line.as_deref(), Some("not applicable to bulk selection"));

    a.status_line = None;
    let collapsed_before = a.collapsed[ListPane::Queue.idx()];
    a.update(key('z'));
    assert_eq!(a.collapsed[ListPane::Queue.idx()], collapsed_before); // unchanged
    assert_eq!(a.status_line.as_deref(), Some("not applicable to bulk selection"));
}

#[test]
fn tasks_bulk_range_refuses_collapse() {
    // TASKS keeps no bulk-doable verb, including `z`.
    let mut snap = StateSnapshot { projects: vec![Project { name: "platform".into(), github_id: None }], ..Default::default() };
    snap.tasks = vec![];
    let mut a = app_with(snap);
    a.defs_by_project.insert("platform".into(), vec![
        { let mut d = crate::ipc::types::DefinitionSummary::default(); d.repo = "platform".into(); d.name = "lint".into(); d },
        { let mut d = crate::ipc::types::DefinitionSummary::default(); d.repo = "platform".into(); d.name = "deploy".into(); d },
    ]);
    a.update(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))); // → tasks
    a.update(shift_down());
    let collapsed_before = a.collapsed[ListPane::Tasks.idx()];
    a.update(key('z'));
    assert_eq!(a.collapsed[ListPane::Tasks.idx()], collapsed_before);
    assert_eq!(a.status_line.as_deref(), Some("not applicable to bulk selection"));
}

#[test]
fn collapse_chip_click_refuses_on_a_bulk_selection() {
    // The title-bar Collapse chip is handled inline in `mouse.rs` (it doesn't
    // route through `apply_action` like the other chips) — a separate wiring
    // point that must carry the same bulk guard as the `z` key.
    let mut a = app_with(two_queue_one_failed());
    a.update(shift_down()); // 2-row bulk range on QUEUE
    let mut hits = a.hit.clone();
    hits.push(Rect { x: 20, y: 0, width: 4, height: 1 }, HitTarget::PaneButton(PaneId::Queue, crate::hit::PaneButton::Collapse));
    a.hit = hits;
    let before = a.collapsed[ListPane::Queue.idx()];
    a.update(mouse(MouseEventKind::Down(MouseButton::Left), 21, 0));
    assert_eq!(a.collapsed[ListPane::Queue.idx()], before, "collapse did not toggle");
    assert_eq!(a.status_line.as_deref(), Some("not applicable to bulk selection"));
}

#[test]
fn esc_with_active_range_clears_range_before_it_can_open_bulk() {
    // Staged Esc (Task 11): first Esc clears the range, so a subsequent `a`
    // opens a single-target menu, not a bulk one.
    let mut a = app_with(two_queue_one_failed());
    a.update(shift_down());
    assert_ne!(a.active_ui().selections[0].anchor, None);
    a.update(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    assert_eq!(a.active_ui().selections[0].anchor, None);
    a.update(key('a'));
    match &a.mode { Mode::ActionMenu { title, .. } => assert_ne!(title, "2 selected"), _ => panic!() }
}

fn six_queue_failed(ids: &[&str]) -> StateSnapshot {
    let tasks = ids
        .iter()
        .map(|id| {
            let mut t = TaskInstance::default();
            t.id = (*id).into();
            t.status = TaskStatus::Failed;
            t.target.repo = "platform".into();
            t
        })
        .collect();
    StateSnapshot { tasks, projects: vec![Project { name: "platform".into(), github_id: None }], ..Default::default() }
}

#[test]
fn bulk_open_does_not_panic_when_rows_empty_between_select_and_open() {
    // Race: a daemon snapshot empties the visible rows AFTER the range is
    // extended but BEFORE `a` opens the menu. The selection anchor/cursor
    // are untouched by a snapshot, so the range guard still fires; the empty
    // visible set must bail (no `vis[0..=0]` panic on an empty slice).
    let mut a = app_with(two_queue_one_failed());
    a.update(shift_down()); // anchor=0, cursor=1 (range of 2)
    // All queue rows vanish while the range is still active.
    a.update(Event::Snapshot(StateSnapshot { tasks: vec![], projects: vec![Project { name: "platform".into(), github_id: None }], ..Default::default() }));
    a.update(key('a')); // must not panic
    // Nothing survives → open bails, menu never opens (status line set instead).
    assert!(matches!(a.mode, Mode::List));
}

#[test]
fn queue_range_requeue_clamps_when_rows_shrink_below_frozen_start() {
    // Race: the range is anchored high (start=3, cursor=5) then the visible set
    // shrinks to 2 rows before `r`. An un-clamped `vis[3..=5]` is an out-of-bounds
    // / inverted-range panic; `queue_selection_rows` clamps the span to the
    // surviving rows instead, so `r` re-queues only what's left (row 1 = a1).
    let mut a = app_with(six_queue_failed(&["a0", "a1", "a2", "a3", "a4", "a5"]));
    a.update(down()); a.update(down()); a.update(down()); // cursor → 3
    a.update(shift_down()); a.update(shift_down()); // anchor=3, cursor=5
    assert_eq!(a.active_ui().selections[0], Selection { cursor: 5, anchor: Some(3) });
    a.update(Event::Snapshot(six_queue_failed(&["a0", "a1"])));
    let u = a.update(key('r')); // must not panic
    // The [3..=5] span clamps against the 2 surviving rows → exactly one row
    // re-queued (a failed, eligible task); no out-of-bounds panic.
    match u.cmds.iter().find(|c| matches!(c, Cmd::RpcSeq { .. })).unwrap() {
        Cmd::RpcSeq { calls, .. } => assert_eq!(calls.len(), 1),
        _ => unreachable!(),
    }
}
