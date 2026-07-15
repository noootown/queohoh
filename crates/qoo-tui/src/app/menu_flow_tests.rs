use super::*;
use crate::event::SessionChoice;
use crate::ipc::types::{Project, SessionEntry, StateSnapshot, TaskInstance, TaskStatus, WorktreeInfo};
use crate::runfiles::RunFiles;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;

fn key(c: char) -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
}
fn enter() -> Event {
    Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
}
fn down() -> Event {
    Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
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

// --- queue `r` / `x` chip-keys (the queue menu's old verbs are now keys) ------

#[test]
fn queue_r_confirms_then_requeues_the_selected_failed_task() {
    // `r` on QUEUE always opens the confirm dialog first (single row); Enter
    // fires the retry via an RpcSeq (verb "reran").
    let mut a = app_with(failed_task_snapshot());
    let opened = a.update(key('r'));
    assert!(opened.cmds.is_empty(), "r opens the dialog, dispatches nothing yet");
    match &a.mode {
        Mode::Confirm { action: ConfirmAction::RequeueTasks { calls }, .. } => {
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].method, "retry");
            assert_eq!(calls[0].params, serde_json::json!({ "id": "t1" }));
        }
        other => panic!("expected requeue confirm, got {other:?}"),
    }
    let u = a.update(enter()); // confirm
    assert!(matches!(a.mode, Mode::List));
    match u.cmds.iter().find(|c| matches!(c, Cmd::RpcSeq { .. })).unwrap() {
        Cmd::RpcSeq { verb, calls, .. } => {
            assert_eq!(verb, "reran");
            assert_eq!(calls[0].method, "retry");
            assert_eq!(calls[0].params, serde_json::json!({ "id": "t1" }));
        }
        _ => unreachable!(),
    }
}

#[test]
fn queue_a_archives_the_selected_terminal_task() {
    // `a` on a live terminal row fires the archive half of the toggle directly
    // (no confirm dialog — unarchive is the built-in undo).
    let mut a = app_with(failed_task_snapshot());
    let u = a.update(key('a'));
    assert!(matches!(a.mode, Mode::List));
    assert!(
        u.cmds.iter().any(|c| matches!(c, Cmd::Rpc { call, .. }
            if call.method == "archive" && call.params == serde_json::json!({ "id": "t1" }))),
        "expected an archive dispatch, got {:?}",
        u.cmds,
    );
}

#[test]
fn queue_a_on_an_archived_row_unarchives_it() {
    // The toggle's other half: `a` on a dimmed archived row restores it.
    let mut t = TaskInstance::default();
    t.id = "t1".into();
    t.status = TaskStatus::Done;
    t.target.repo = "platform".into();
    let snap = StateSnapshot {
        projects: vec![Project { name: "platform".into(), github_id: None }],
        archived_recent: vec![t],
        ..Default::default()
    };
    let mut a = app_with(snap);
    let u = a.update(key('a'));
    assert!(
        u.cmds.iter().any(|c| matches!(c, Cmd::Rpc { call, .. }
            if call.method == "unarchive" && call.params == serde_json::json!({ "id": "t1" }))),
        "expected an unarchive dispatch, got {:?}",
        u.cmds,
    );
}

#[test]
fn queue_a_refuses_active_rows_with_a_status_line() {
    // Archiving live work (queued/running/needs-input) is refused locally.
    let mut snap = failed_task_snapshot();
    snap.tasks[0].status = TaskStatus::Running;
    let mut a = app_with(snap);
    let u = a.update(key('a'));
    assert!(u.cmds.is_empty(), "no RPC for an active row");
    assert_eq!(a.status_line.as_deref(), Some("cannot archive a running task"));
}

// --- bulk archive helpers -------------------------------------------------
fn terminal_task(id: &str, status: TaskStatus) -> TaskInstance {
    let mut t = TaskInstance::default();
    t.id = id.into();
    t.status = status;
    t.target.repo = "platform".into();
    t
}
fn rpcseq_methods<'a>(cmds: &'a [Cmd], want_verb: &str) -> Vec<(String, String)> {
    // (method, id) pairs for the single RpcSeq matching `want_verb`.
    cmds.iter()
        .find_map(|c| match c {
            Cmd::RpcSeq { verb, calls, .. } if verb == want_verb => Some(
                calls
                    .iter()
                    .map(|c| (c.method.clone(), c.params["id"].as_str().unwrap_or("").to_string()))
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

#[test]
fn queue_a_bulk_archives_every_terminal_row() {
    // A range of two live terminal rows: `a` fans one `archive` out per row
    // through an RpcSeq (verb "archived"), no confirm — mirrors the bulk
    // stop/rerun path.
    let snap = StateSnapshot {
        tasks: vec![terminal_task("t1", TaskStatus::Failed), terminal_task("t2", TaskStatus::Done)],
        projects: vec![Project { name: "platform".into(), github_id: None }],
        ..Default::default()
    };
    let mut a = app_with(snap);
    a.update(shift_down()); // extend into a 2-row range
    let u = a.update(key('a'));
    let calls = rpcseq_methods(&u.cmds, "archived");
    assert!(calls.iter().all(|(m, _)| m == "archive"), "all archive: {calls:?}");
    let ids: std::collections::HashSet<&str> = calls.iter().map(|(_, id)| id.as_str()).collect();
    assert_eq!(ids, ["t1", "t2"].into_iter().collect(), "both rows archived: {calls:?}");
}

#[test]
fn queue_a_bulk_unarchives_when_first_selected_is_archived() {
    // Direction is fixed by the first (topmost) selected row. A range over two
    // archived rows (nothing live) unarchives every one (verb "unarchived").
    let snap = StateSnapshot {
        projects: vec![Project { name: "platform".into(), github_id: None }],
        archived_recent: vec![
            terminal_task("t1", TaskStatus::Done),
            terminal_task("t2", TaskStatus::Done),
        ],
        ..Default::default()
    };
    let mut a = app_with(snap);
    a.update(shift_down());
    let u = a.update(key('a'));
    let calls = rpcseq_methods(&u.cmds, "unarchived");
    assert!(calls.iter().all(|(m, _)| m == "unarchive"), "all unarchive: {calls:?}");
    let ids: std::collections::HashSet<&str> = calls.iter().map(|(_, id)| id.as_str()).collect();
    assert_eq!(ids, ["t1", "t2"].into_iter().collect(), "both rows unarchived: {calls:?}");
}

#[test]
fn queue_a_bulk_archive_skips_active_rows() {
    // Archive direction (first selected is a live running row) archives the
    // terminal rows and skips the active one — active work is never hidden.
    let snap = StateSnapshot {
        tasks: vec![
            terminal_task("run", TaskStatus::Running),
            terminal_task("done", TaskStatus::Failed),
        ],
        projects: vec![Project { name: "platform".into(), github_id: None }],
        ..Default::default()
    };
    let mut a = app_with(snap);
    a.update(shift_down()); // running (ACTIVE, topmost) + failed (FINISHED)
    let u = a.update(key('a'));
    let calls = rpcseq_methods(&u.cmds, "archived");
    assert_eq!(calls, vec![("archive".into(), "done".into())], "only the terminal row: {calls:?}");
}

#[test]
fn queue_a_bulk_sets_status_line_when_nothing_eligible() {
    // A range with only active rows has nothing to archive — no RPC, a status
    // line instead (parity with bulk stop/rerun).
    let snap = StateSnapshot {
        tasks: vec![
            terminal_task("r1", TaskStatus::Running),
            terminal_task("r2", TaskStatus::Running),
        ],
        projects: vec![Project { name: "platform".into(), github_id: None }],
        ..Default::default()
    };
    let mut a = app_with(snap);
    a.update(shift_down());
    let u = a.update(key('a'));
    assert!(u.cmds.is_empty(), "no RPC when nothing is eligible");
    assert_eq!(a.status_line.as_deref(), Some("nothing to archive in selection"));
}

#[test]
fn queue_r_noop_on_running_sets_status_line() {
    let mut snap = failed_task_snapshot();
    snap.tasks[0].status = TaskStatus::Running;
    let mut a = app_with(snap);
    let u = a.update(key('r'));
    assert!(u.cmds.is_empty(), "running is not re-queueable");
    assert!(a.status_line.as_deref().unwrap_or("").contains("rerun"), "status: {:?}", a.status_line);
}

#[test]
fn queue_r_confirms_on_cancelled_task() {
    // A user-cancelled task is rerunnable: `r` opens the confirm dialog (no
    // status-line no-op) and Enter fires the retry.
    let mut snap = failed_task_snapshot();
    snap.tasks[0].status = TaskStatus::Cancelled;
    let mut a = app_with(snap);
    let opened = a.update(key('r'));
    assert!(opened.cmds.is_empty(), "r opens the dialog, dispatches nothing yet");
    assert!(
        matches!(a.mode, Mode::Confirm { action: ConfirmAction::RequeueTasks { .. }, .. }),
        "cancelled task should open the rerun confirm, got {:?}", a.mode,
    );
    let u = a.update(enter()); // confirm
    assert!(u.cmds.iter().any(|c| matches!(c, Cmd::RpcSeq { calls, .. } if calls[0].method == "retry")));
}

#[test]
fn queue_r_confirms_on_every_non_running_status() {
    // Rerun is allowed in ANY status except running (whose in-flight worker
    // owns the row — stop it first): done, skipped, and even queued (an
    // idempotent no-op daemon-side) all open the confirm dialog.
    for status in [TaskStatus::Done, TaskStatus::Skipped, TaskStatus::Queued] {
        let mut snap = failed_task_snapshot();
        snap.tasks[0].status = status;
        let mut a = app_with(snap);
        a.update(key('r'));
        assert!(
            matches!(a.mode, Mode::Confirm { action: ConfirmAction::RequeueTasks { .. }, .. }),
            "{status:?} task should open the rerun confirm, got {:?}", a.mode,
        );
    }
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
    // A done (terminal) task can't be stopped: status line, no dialog, no cmd.
    let mut snap = failed_task_snapshot();
    snap.tasks[0].status = TaskStatus::Done;
    let mut a = app_with(snap);
    let u = a.update(key('x'));
    assert!(matches!(a.mode, Mode::List), "no dialog when nothing is stoppable");
    assert!(u.cmds.is_empty());
    assert!(a.status_line.as_deref().unwrap_or("").contains("stop"), "status: {:?}", a.status_line);
}

#[test]
fn queue_needs_input_is_requeueable_and_cancellable_via_skip() {
    // Needs-input: `r` confirms then re-queues (retry); `x` confirms then skips.
    let mut snap = failed_task_snapshot();
    snap.tasks[0].status = TaskStatus::NeedsInput;
    let mut a = app_with(snap);
    a.update(key('r'));
    assert!(matches!(a.mode, Mode::Confirm { action: ConfirmAction::RequeueTasks { .. }, .. }));
    let ru = a.update(enter());
    assert!(ru.cmds.iter().any(|c| matches!(c, Cmd::RpcSeq { calls, .. } if calls[0].method == "retry")));

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
    // A range of only running rows (the ONE ineligible status) has nothing to
    // re-queue.
    let mut t0 = TaskInstance::default(); t0.id = "t0".into(); t0.status = TaskStatus::Running; t0.target.repo = "platform".into();
    let mut t1 = TaskInstance::default(); t1.id = "t1".into(); t1.status = TaskStatus::Running; t1.target.repo = "platform".into();
    let snap = StateSnapshot { tasks: vec![t0, t1], projects: vec![Project { name: "platform".into(), github_id: None }], ..Default::default() };
    let mut a = app_with(snap);
    a.update(shift_down());
    let u = a.update(key('r'));
    assert!(u.cmds.is_empty());
    assert!(a.status_line.as_deref().unwrap_or("").contains("rerunnable"), "status: {:?}", a.status_line);
}

#[test]
fn queue_range_requeue_all_eligible_requeues_each() {
    // A range of two failed tasks confirms first, then re-queues both in one RpcSeq.
    let mut t0 = TaskInstance::default(); t0.id = "t0".into(); t0.status = TaskStatus::Failed; t0.target.repo = "platform".into();
    let mut t1 = TaskInstance::default(); t1.id = "t1".into(); t1.status = TaskStatus::Failed; t1.target.repo = "platform".into();
    let snap = StateSnapshot { tasks: vec![t0, t1], projects: vec![Project { name: "platform".into(), github_id: None }], ..Default::default() };
    let mut a = app_with(snap);
    a.update(shift_down());
    a.update(key('r')); // opens confirm
    assert!(matches!(a.mode, Mode::Confirm { action: ConfirmAction::RequeueTasks { .. }, .. }));
    let u = a.update(enter()); // confirm
    match u.cmds.iter().find(|c| matches!(c, Cmd::RpcSeq { .. })).unwrap() {
        Cmd::RpcSeq { verb, calls, .. } => {
            assert_eq!(verb, "reran");
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
    // A range of only terminal rows has nothing to stop.
    let mut t0 = TaskInstance::default(); t0.id = "t0".into(); t0.status = TaskStatus::Done; t0.target.repo = "platform".into();
    let mut t1 = TaskInstance::default(); t1.id = "t1".into(); t1.status = TaskStatus::Failed; t1.target.repo = "platform".into();
    let snap = StateSnapshot { tasks: vec![t0, t1], projects: vec![Project { name: "platform".into(), github_id: None }], ..Default::default() };
    let mut a = app_with(snap);
    a.update(shift_down());
    let u = a.update(key('x'));
    assert!(matches!(a.mode, Mode::List), "no dialog when nothing is stoppable");
    assert!(u.cmds.is_empty());
    assert!(a.status_line.as_deref().unwrap_or("").contains("stop"), "status: {:?}", a.status_line);
}

// --- queue `g` (goto): the retired single-target Resume menu's verb, now a
// direct key — mirrors the worktrees `g_on_worktree_row_*` tests below. -------

#[test]
fn queue_g_noop_outside_tmux_sets_status() {
    let mut a = app_with(failed_task_snapshot());
    a.inside_tmux = false;
    let up = a.update(key('g'));
    assert!(up.cmds.is_empty(), "no tmux → no TmuxResume");
    assert!(a.status_line.as_deref().unwrap_or("").contains("tmux"), "status: {:?}", a.status_line);
}

#[test]
fn queue_g_no_session_sets_status_when_task_never_ran() {
    // Reason precedence: tmux first, then session, then worktree. The fixture
    // task has no `resume_session_id` and no run record → "no session yet".
    let mut a = app_with(failed_task_snapshot());
    a.inside_tmux = true;
    let up = a.update(key('g'));
    assert!(up.cmds.is_empty());
    assert!(
        a.status_line.as_deref().unwrap_or("").contains("no session"),
        "status: {:?}", a.status_line
    );
}

#[test]
fn queue_g_no_worktree_sets_status_when_session_exists_but_no_worktree() {
    // A session id is recorded but the task has no worktree target and no run
    // record with a path → the second data gap ("no worktree for this task").
    let mut snap = failed_task_snapshot();
    snap.tasks[0].resume_session_id = Some("sess-x".into());
    let mut a = app_with(snap);
    a.inside_tmux = true;
    let up = a.update(key('g'));
    assert!(up.cmds.is_empty());
    assert!(
        a.status_line.as_deref().unwrap_or("").contains("no worktree"),
        "status: {:?}", a.status_line
    );
}

#[test]
fn queue_g_resumes_the_selected_tasks_session() {
    // Happy path: a run record with both a session id and a worktree path
    // (keyed to the selected task) resumes it via `Cmd::TmuxResume`.
    let mut a = app_with(failed_task_snapshot());
    a.inside_tmux = true;
    a.run_files = Some((
        "t1".to_string(),
        Box::new(RunFiles {
            session_id: Some("sess-flaky".into()),
            worktree_path: Some("/repos/acme-flaky".into()),
            ..Default::default()
        }),
    ));
    let up = a.update(key('g'));
    assert!(matches!(&up.cmds[..], [Cmd::TmuxResume { path, session_id }]
        if path == "/repos/acme-flaky" && session_id == "sess-flaky"));
}

#[test]
fn queue_g_bulk_range_refuses_not_applicable() {
    // A multi-row range on QUEUE's `g` isn't bulk-doable (only rerun/stop are)
    // → refuses with a status line instead of acting on the cursor row alone.
    let mut t0 = TaskInstance::default();
    t0.id = "t0".into();
    t0.status = TaskStatus::Failed;
    t0.target.repo = "platform".into();
    let mut t1 = TaskInstance::default();
    t1.id = "t1".into();
    t1.status = TaskStatus::Failed;
    t1.target.repo = "platform".into();
    let snap = StateSnapshot {
        tasks: vec![t0, t1],
        projects: vec![Project { name: "platform".into(), github_id: None }],
        ..Default::default()
    };
    let mut a = app_with(snap);
    a.inside_tmux = true;
    a.update(shift_down());
    let up = a.update(key('g'));
    assert!(up.cmds.is_empty());
    assert_eq!(a.status_line.as_deref(), Some("not applicable to bulk selection"));
}

// --- worktrees `r`/`g`/`x` hotkeys (replace the retired worktree menu) --------

// --- session picker (`r` on a worktree row) -----------------------------------

/// Two newest-first loaded sessions for `worktree`, mirroring the `listSessions`
/// reply shape.
fn loaded(worktree: &str) -> Event {
    Event::SessionsLoaded {
        worktree: worktree.into(),
        result: Ok(vec![
            SessionChoice { session_id: "sess-1".into(), label: "Fix the parser".into(), mtime_ms: 2_000, model: Some("sonnet".into()) },
            SessionChoice { session_id: "sess-2".into(), label: "Redesign TUI".into(), mtime_ms: 1_000, model: None },
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
    // The launch form pins the session; the label rides in the form title; and
    // the model dropdown defaults to the model that session already ran on.
    match &a.mode {
        Mode::Form { state, action: FormAction::NewSession { resume_session_id: Some(s), .. } } => {
            assert_eq!(s, "sess-1");
            assert!(state.title.contains("Fix the parser"), "title: {}", state.title);
            assert_eq!(state.fields[0].value, "sonnet", "model defaults to the resumed session's model");
        }
        other => panic!("expected NewSession resume form, got {other:?}"),
    }
}

#[test]
fn resuming_a_session_without_a_recorded_model_falls_back_to_default() {
    // sess-2 has `model: None` (e.g. started outside queohoh); the resume form
    // then falls back to the resolved default (opus, no settings fetched).
    let mut a = app_with(worktree_snapshot());
    focus_worktrees(&mut a);
    a.update(key('r'));
    a.update(loaded("platform.wt-a"));
    // Rows: New(0), Create Worktree(1), sess-1(2), sess-2(3) — three downs.
    a.update(down());
    a.update(down());
    a.update(down());
    a.update(enter());
    match &a.mode {
        Mode::Form { state, action: FormAction::NewSession { resume_session_id: Some(s), .. } } => {
            assert_eq!(s, "sess-2");
            assert_eq!(state.fields[0].value, "opus", "no recorded model → resolved default");
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
fn a_no_longer_opens_a_menu_on_queue() {
    // `a` retired the queue's single-target Resume menu too — its verb moved
    // to `g` (see the `queue_g_*` tests above).
    let mut a = app_with(failed_task_snapshot());
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
fn tasks_pane_d_dispatches_discover_for_a_discovery_def() {
    let snap = StateSnapshot {
        projects: vec![Project { name: "platform".into(), github_id: None }],
        ..Default::default()
    };
    let mut a = app_with(snap);
    a.defs_by_project.insert("platform".into(), vec![{
        let mut d = crate::ipc::types::DefinitionSummary::default();
        d.repo = "platform".into();
        d.name = "pr-review".into();
        d.has_discovery = true;
        d
    }]);
    focus_tasks(&mut a);
    let u = a.update(key('d'));
    assert!(matches!(a.mode, Mode::List));
    assert!(
        u.cmds.iter().any(|c| matches!(c, Cmd::Rpc { call, invalidate_defs_for, .. }
            if call.method == "discoverDefinition"
                && call.params["name"] == "pr-review"
                && call.params["source"] == "tui"
                && invalidate_defs_for.as_deref() == Some("platform"))),
        "expected a discoverDefinition dispatch, got {:?}",
        u.cmds,
    );
    // The optimistic in-flight marker drives the `⌕`-spinner (and the tick
    // that animates it) until the repo's def refetch lands.
    assert!(a.discovering.contains("platform/pr-review"));
    assert!(a.wants_tick(), "an in-flight discover must keep the tick alive for the throbber");
}

#[test]
fn discover_spinner_clears_when_repo_defs_refetch_lands() {
    let snap = StateSnapshot {
        projects: vec![Project { name: "platform".into(), github_id: None }],
        ..Default::default()
    };
    let mut a = app_with(snap);
    a.discovering.insert("platform/pr-review".into());
    a.discovering.insert("other/pr-review".into());
    // The discover RPC's ActionResult invalidates + refetches the repo's defs;
    // the landing `Definitions` event is the single clear point (it fires on
    // success, RPC error, AND client timeout — so the spinner can't stick).
    a.update(Event::Definitions { repo: "platform".into(), defs: vec![] });
    assert!(!a.discovering.contains("platform/pr-review"), "landed refetch stops the spinner");
    assert!(
        a.discovering.contains("other/pr-review"),
        "another repo's in-flight discover is untouched"
    );
}

#[test]
fn tasks_pane_d_on_a_no_discovery_def_sets_status_line_no_rpc() {
    let snap = StateSnapshot {
        projects: vec![Project { name: "platform".into(), github_id: None }],
        ..Default::default()
    };
    let mut a = app_with(snap);
    a.defs_by_project.insert("platform".into(), vec![{
        let mut d = crate::ipc::types::DefinitionSummary::default();
        d.repo = "platform".into();
        d.name = "lint".into();
        // has_discovery: false (default)
        d
    }]);
    focus_tasks(&mut a);
    let u = a.update(key('d'));
    assert!(u.cmds.is_empty(), "no RPC for a def without discovery");
    assert_eq!(a.status_line.as_deref(), Some("lint has no discovery"));
    assert!(a.discovering.is_empty(), "a refused discover must not start the spinner");
}

