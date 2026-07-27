//! Lazy full-task fetch for the DETAIL Prompt tab (untruncated prompt via `task` RPC).

use super::*;
use crate::ipc::types::{Project, StateSnapshot, TaskInstance, TaskStatus, TaskTarget};
use crate::test_fixtures::fixture_app;

fn truncated_task(id: &str, prompt_body: &str) -> TaskInstance {
    // Mirror the daemon's forWire: body clipped then `…` suffix.
    let mut t = TaskInstance {
        id: id.into(),
        status: TaskStatus::Queued,
        target: TaskTarget { repo: "acme".into(), git_ref: "main".into(), worktree: None },
        prompt: format!("{prompt_body}…"),
        ..Default::default()
    };
    t.created = "2026-07-09T12:00:00.000Z".into();
    t.priority = "normal".into();
    t.source = "tui".into();
    t.session = "fresh".into();
    t
}

fn app_on_prompt_tab_with_truncated(id: &str, body: &str) -> App {
    let mut app = fixture_app();
    // Replace fixture tasks with one truncated-prompt task selected on Queue.
    app.snapshot = Some(StateSnapshot {
        projects: vec![Project { name: "acme".into(), github_id: None }],
        tasks: vec![truncated_task(id, body)],
        ..Default::default()
    });
    app.snapshot_gen = 1;
    let mut ui = TabUiState::default();
    ui.last_list_pane = ListPane::Queue;
    ui.sub_tab[DetailKind::Run as usize] = crate::detail::RUN_TAB_PROMPT;
    app.ui_by_tab.insert("acme".into(), ui);
    app
}

#[test]
fn reconcile_full_task_fetches_once_and_caches_on_reply() {
    let mut app = app_on_prompt_tab_with_truncated("t1", &"x".repeat(50));
    let cmd = app.reconcile_full_task();
    assert!(matches!(cmd, Some(Cmd::FetchTask { ref id }) if id == "t1"));
    assert!(app.full_tasks_inflight.contains("t1"));
    assert!(app.reconcile_full_task().is_none(), "in-flight fetch dedups");

    let mut full = truncated_task("t1", &"x".repeat(50));
    full.prompt = format!("{} and the rest of the long prompt", "x".repeat(50));
    app.update(Event::Task { id: "t1".into(), task: Some(Box::new(full.clone())) });
    assert_eq!(app.full_tasks.get("t1").map(|t| t.prompt.as_str()), Some(full.prompt.as_str()));
    assert!(!app.full_tasks_inflight.contains("t1"));
    assert!(app.reconcile_full_task().is_none(), "cached task is not refetched");
}

#[test]
fn reconcile_full_task_skips_when_wire_prompt_not_truncated() {
    let mut app = fixture_app();
    // fixture prompts are short and have no `…` — Prompt tab must not fetch.
    let mut ui = TabUiState::default();
    ui.last_list_pane = ListPane::Queue;
    ui.sub_tab[DetailKind::Run as usize] = crate::detail::RUN_TAB_PROMPT;
    app.ui_by_tab.insert("acme".into(), ui);
    assert!(app.reconcile_full_task().is_none());
}

#[test]
fn reconcile_full_task_only_on_prompt_sub_tab() {
    let mut app = app_on_prompt_tab_with_truncated("t1", "long body");
    // Report / transcript / info: no fetch.
    for sub in [
        crate::detail::RUN_TAB_REPORT,
        crate::detail::RUN_TAB_TRANSCRIPT,
        3, // info
    ] {
        app.ui().sub_tab[DetailKind::Run as usize] = sub;
        assert!(
            app.reconcile_full_task().is_none(),
            "sub_tab {sub} must not fetch full task"
        );
    }
    app.ui().sub_tab[DetailKind::Run as usize] = crate::detail::RUN_TAB_PROMPT;
    assert!(app.reconcile_full_task().is_some());
}

#[test]
fn failed_task_fetch_poisons_so_reconcile_does_not_loop() {
    let mut app = app_on_prompt_tab_with_truncated("t1", "body");
    assert!(app.reconcile_full_task().is_some());
    app.update(Event::Task { id: "t1".into(), task: None });
    assert!(app.full_tasks.is_empty());
    assert!(
        app.full_tasks_inflight.contains("t1"),
        "failed fetch leaves poison marker"
    );
    assert!(app.reconcile_full_task().is_none(), "poison must not refetch-loop");
}

#[test]
fn ensure_full_task_dedups_inflight_and_cache() {
    let mut app = App::new("/tmp/runs".into(), "/tmp/s.sock".into());
    let cmds = app.ensure_full_task("abc");
    assert!(matches!(cmds.as_slice(), [Cmd::FetchTask { id }] if id == "abc"));
    assert!(app.ensure_full_task("abc").is_empty(), "inflight dedups");
    app.full_tasks.insert("abc".into(), TaskInstance { id: "abc".into(), ..Default::default() });
    app.full_tasks_inflight.remove("abc");
    assert!(app.ensure_full_task("abc").is_empty(), "cached dedups");
}
