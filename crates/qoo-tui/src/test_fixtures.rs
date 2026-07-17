//! Shared test fixtures: a representative `StateSnapshot` + a ready `App` for
//! render/snapshot tests across the view and app modules.
#![cfg(test)]

use std::collections::HashMap;

use crate::app::App;
use crate::ipc::types::{
    Project, SessionEntry, StateSnapshot, TaskInstance, TaskStatus, TaskTarget, WorktreeInfo,
};

#[allow(clippy::too_many_arguments)]
fn task(
    id: &str,
    status: TaskStatus,
    repo: &str,
    worktree: Option<&str>,
    prompt: &str,
    session: &str,
    created: &str,
    finished: Option<&str>,
) -> TaskInstance {
    TaskInstance {
        id: id.to_string(),
        status,
        definition: None,
        item: None,
        item_key: None,
        target: TaskTarget {
            repo: repo.to_string(),
            git_ref: worktree
                .map(|w| format!("worktree:{w}"))
                .unwrap_or_else(|| "main".to_string()),
            worktree: worktree.map(str::to_string),
        },
        priority: "normal".to_string(),
        created: created.to_string(),
        started_at: None,
        finished_at: finished.map(str::to_string),
        source: "tui".to_string(),
        ephemeral_worktree: false,
        error: None,
        session: session.to_string(),
        resume_session_id: None,
        model: None,
        prompt: prompt.to_string(),
        verify: None,
        verified: None,
        verify_exit_code: None,
        verify_output: None,
    }
}

/// A snapshot with one project, four queue tasks (running/queued/failed live +
/// one archived), two worktrees, and a main session. `created` timestamps are
/// fixed ISO strings so elapsed labels are deterministic against `now_epoch_s`.
pub fn fixture_snapshot() -> StateSnapshot {
    let tasks = vec![
        {
            let mut t = task(
                "01RUN",
                TaskStatus::Running,
                "acme",
                Some("acme.feature"),
                "implement the widget cache",
                "main",
                "2026-07-09T12:00:00.000Z",
                None,
            );
            t.definition = Some("squash-merge".to_string());
            t
        },
        task(
            "01QUE",
            TaskStatus::Queued,
            "acme",
            Some("acme.feature"),
            "write docs for the cache",
            "fresh",
            "2026-07-09T12:04:00.000Z",
            None,
        ),
        {
            let mut t = task(
                "01FAIL",
                TaskStatus::Failed,
                "acme",
                None,
                "flaky migration",
                "fresh",
                "2026-07-09T11:50:00.000Z",
                // Finished after the archived task below → sorts above it in the
                // FINISHED section (most recently finished first).
                Some("2026-07-09T11:52:00.000Z"),
            );
            t.error = Some("exit code 1".to_string());
            t
        },
    ];
    let archived = vec![task(
        "01OLD",
        TaskStatus::Done,
        "acme",
        None,
        "earlier cleanup task",
        "fresh",
        "2026-07-09T10:00:00.000Z",
        Some("2026-07-09T10:05:00.000Z"),
    )];
    let mut worktrees: HashMap<String, Vec<WorktreeInfo>> = HashMap::new();
    worktrees.insert(
        "acme".to_string(),
        vec![
            WorktreeInfo {
                name: "acme.feature".to_string(),
                path: "/repos/acme.feature".to_string(),
                branch: "feature/JB-1200-cache".to_string(),
                ..Default::default()
            },
            WorktreeInfo {
                name: "acme.hotfix".to_string(),
                path: "/repos/acme.hotfix".to_string(),
                branch: "hotfix/login".to_string(),
                ..Default::default()
            },
        ],
    );
    StateSnapshot {
        tasks,
        archived_recent: archived,
        sessions: vec![SessionEntry {
            kind: "interactive".to_string(),
            key: "acme:acme.feature".to_string(),
            lane: Some("acme:acme.feature".to_string()),
            cwd: Some("/repos/acme.feature".to_string()),
            pid: Some(4242),
            started_at: "2026-07-09T11:59:00.000Z".to_string(),
            heartbeat_at: "2026-07-09T12:05:00.000Z".to_string(),
        }],
        running: vec!["01RUN".to_string()],
        max_concurrent: Some(3),
        projects: vec![Project {
            name: "acme".to_string(),
            github_id: None,
        }],
        worktrees,
        build_id: Some("build-1".to_string()),
        active_provider: Some("grok".to_string()),
    }
}

/// App seeded with the fixture snapshot, connected, at a fixed `now_epoch_s`
/// (2026-07-09T12:05:03Z → 5m03s elapsed for the running task).
pub fn fixture_app() -> App {
    let mut app = App::new(
        std::path::PathBuf::from("/tmp/qoo-runs"),
        std::path::PathBuf::from("/tmp/qoo.sock"),
    );
    app.snapshot = Some(fixture_snapshot());
    app.connected = true;
    app.now_epoch_s = 1_752_062_703; // 2026-07-09T12:05:03Z
    app.size = (80, 24);
    app
}
